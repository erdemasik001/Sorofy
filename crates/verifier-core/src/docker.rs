//! Thin wrapper over the `docker` CLI.
//!
//! Source enters and artifacts leave the container as tar streams over
//! stdin/stdout (`docker cp -`), never as a bind mount. Two reasons:
//!
//! 1. No host path ever crosses into the container, so the same code works
//!    whether the daemon is local (Linux deploy target) or reached through
//!    `wsl -- docker` (Windows dev box), where a Windows path would be
//!    meaningless to the daemon.
//! 2. A bind mount is a hole in the sandbox. Untrusted source builds against a
//!    container-private filesystem instead.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use wait_timeout::ChildExt;

use crate::error::{Result, VerifyError};

/// How to invoke the `docker` CLI.
pub struct Docker {
    program: String,
    /// Args placed before the docker subcommand, e.g. `-d Ubuntu -- docker`.
    prefix: Vec<String>,
}

impl Default for Docker {
    fn default() -> Self {
        Self::autodetect()
    }
}

impl Docker {
    /// Invoke `docker` directly (the Linux deploy target).
    pub fn local() -> Self {
        Docker {
            program: "docker".into(),
            prefix: vec![],
        }
    }

    /// Reach the daemon inside a WSL2 distro from a Windows host.
    ///
    /// Docker Desktop is not usable on the dev machine, so Docker Engine runs
    /// natively inside Ubuntu and we shell into it.
    pub fn wsl(distro: &str) -> Self {
        Docker {
            program: "wsl".into(),
            prefix: vec!["-d".into(), distro.into(), "--".into(), "docker".into()],
        }
    }

    /// Pick a runner for the current host.
    ///
    /// `VERIFY_DOCKER` overrides the whole command line (e.g.
    /// `VERIFY_DOCKER="wsl -d Ubuntu -- docker"`, or `podman` on a host that
    /// prefers it).
    pub fn autodetect() -> Self {
        if let Ok(spec) = std::env::var("VERIFY_DOCKER") {
            let mut parts = spec.split_whitespace().map(String::from);
            if let Some(program) = parts.next() {
                return Docker {
                    program,
                    prefix: parts.collect(),
                };
            }
        }
        if cfg!(windows) {
            Docker::wsl("Ubuntu")
        } else {
            Docker::local()
        }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.prefix);
        cmd
    }

    /// Run a docker subcommand to completion and return its stdout.
    fn run(&self, args: &[&str]) -> Result<Vec<u8>> {
        let out =
            self.command().args(args).output().map_err(|e| {
                VerifyError::Docker(format!("could not run `{}`: {e}", self.program))
            })?;
        if !out.status.success() {
            return Err(VerifyError::Docker(format!(
                "`docker {}` failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(out.stdout)
    }

    /// Fail early with a clear message if the daemon is unreachable.
    pub fn preflight(&self) -> Result<()> {
        self.run(&["version", "--format", "{{.Server.Version}}"])
            .map(|_| ())
    }

    /// Resolve an image's `repo@sha256:...` digest, pulling it if absent.
    ///
    /// This is how a `bldimg` is minted for an image we build locally, and how
    /// we confirm a submitted digest actually resolves.
    pub fn image_digest(&self, image: &str) -> Result<Option<String>> {
        let raw = self.run(&[
            "image",
            "inspect",
            image,
            "--format",
            "{{json .RepoDigests}}",
        ])?;
        let digests: Vec<String> = serde_json::from_slice(&raw)
            .map_err(|e| VerifyError::Docker(format!("could not parse RepoDigests: {e}")))?;
        Ok(digests.into_iter().next())
    }

    /// Create a stopped container. `argv` is passed to the image's entrypoint.
    pub fn create(&self, spec: &ContainerSpec<'_>) -> Result<Container<'_>> {
        let args = create_args(spec);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = self.run(&refs)?;
        let id = String::from_utf8_lossy(&out).trim().to_string();
        if id.is_empty() {
            return Err(VerifyError::Docker(
                "`docker create` returned no container id".into(),
            ));
        }
        Ok(Container { docker: self, id })
    }

    /// Create a named docker-managed volume, removed on drop.
    ///
    /// Docker-managed rather than a host directory: it needs to be writable by
    /// the daemon wherever it runs, including across the WSL boundary where a
    /// Windows path would mean nothing.
    pub fn create_volume(&self, name: &str) -> Result<Volume<'_>> {
        self.run(&["volume", "create", name])?;
        Ok(Volume {
            docker: self,
            name: name.to_string(),
        })
    }
}

/// Whether a container can reach the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network<'a> {
    /// Default bridge networking.
    Bridge,
    /// `--network=none`.
    None,
    /// A named docker network, `--network=<name>`.
    ///
    /// The seam for egress control (docs/adr/0001-fetch-egress-control.md, G4-b):
    /// the fetch phase needs egress, but must not reach internal addresses an
    /// attacker-controlled `Cargo.lock` names. Putting it on a dedicated network
    /// gives the host firewall a stable subnet to filter. The network must already
    /// exist — `docker create` fails otherwise, which is deliberate: silently
    /// falling back to the default bridge would mean an unfiltered egress path
    /// while the config claims otherwise.
    Named(&'a str),
}

/// cgroup limits for a container. A `None` field imposes no limit.
///
/// Set on the build (and fetch) containers so a hostile `build.rs` cannot OOM
/// the host, fork-bomb it, or peg every core. The wall-clock timeout bounds
/// *time*, not *resources* — these bound the resources (docs/security.md, G1).
#[derive(Debug, Clone, Copy, Default)]
pub struct ResourceLimits<'a> {
    /// `--memory`, e.g. `"3g"`.
    pub memory: Option<&'a str>,
    /// `--memory-swap`, e.g. `"3g"`. Set equal to `memory` to disable swap:
    /// without it, a swap-enabled host lifts the effective memory ceiling to
    /// ~2×. Only emitted alongside `memory` — docker requires `--memory` to be
    /// set for `--memory-swap` to mean anything.
    pub memory_swap: Option<&'a str>,
    /// `--cpus`, e.g. `"2"`.
    pub cpus: Option<&'a str>,
    /// `--pids-limit`, e.g. `2048`.
    pub pids: Option<u32>,
    /// `--storage-opt size=`, e.g. `"10g"`: caps the container's writable layer
    /// so a build cannot fill the host disk. Left unset by default because the
    /// flag requires a quota-capable storage driver (overlay2 on xfs with
    /// pquota, or btrfs/zfs/devicemapper); the daemon rejects it on a driver
    /// without quota support. Enable per-deploy once the driver is known good.
    pub storage_opt_size: Option<&'a str>,
}

/// Container hardening flags. A `false` field emits nothing.
///
/// Distinct from [`ResourceLimits`] (cgroup *amounts*): these harden the
/// container's *privileges*. Set on a sandbox that compiles untrusted code so a
/// `build.rs` cannot lean on Linux capabilities or gain privileges via setuid
/// (docs/security.md, G6). Read-only rootfs is a further follow-up that would
/// live here.
#[derive(Debug, Clone, Copy, Default)]
pub struct SecurityOpts {
    /// `--cap-drop=ALL`: drop every Linux capability. A compile needs none.
    pub cap_drop_all: bool,
    /// `--security-opt=no-new-privileges`: block gaining privileges through a
    /// setuid/setgid binary. The build already runs non-root (uid 1000).
    pub no_new_privileges: bool,
}

/// What to create a build container from.
pub struct ContainerSpec<'a> {
    pub image: &'a str,
    /// Override the image's entrypoint (our image entrypoints to `stellar`).
    pub entrypoint: Option<&'a str>,
    pub argv: &'a [String],
    pub workdir: &'a str,
    pub env: &'a [(&'a str, &'a str)],
    /// `(volume_name, mount_path)` pairs.
    pub volumes: &'a [(&'a str, &'a str)],
    pub network: Network<'a>,
    /// cgroup caps for the container (memory/CPU/PIDs).
    pub limits: ResourceLimits<'a>,
    /// Capability/privilege hardening for the container.
    pub security: SecurityOpts,
}

/// Build the `docker create ...` argument vector for a spec.
///
/// Split out from [`Docker::create`] so the flag construction — including the
/// resource limits that keep an untrusted build from exhausting the host — can
/// be unit-tested without a running daemon.
fn create_args(spec: &ContainerSpec<'_>) -> Vec<String> {
    let mut args: Vec<String> = vec!["create".into()];
    match spec.network {
        // Bridge is the daemon default, so it emits no flag.
        Network::Bridge => {}
        Network::None => args.push("--network=none".into()),
        Network::Named(name) => args.push(format!("--network={name}")),
    }
    // Privilege hardening (docs/security.md, G6): a sandbox that compiles
    // untrusted code needs no capabilities and must not let a setuid binary
    // escalate. Emitted next to --network as they are the same isolation posture.
    if spec.security.cap_drop_all {
        args.push("--cap-drop=ALL".into());
    }
    if spec.security.no_new_privileges {
        args.push("--security-opt=no-new-privileges".into());
    }
    // Resource caps first: a build compiles and runs untrusted code, so bound
    // what one job can take from the host.
    if let Some(memory) = spec.limits.memory {
        args.push("--memory".into());
        args.push(memory.into());
        // Nested under --memory: docker requires --memory for --memory-swap to
        // have meaning, so a swap cap without a memory cap is never emitted.
        if let Some(memory_swap) = spec.limits.memory_swap {
            args.push("--memory-swap".into());
            args.push(memory_swap.into());
        }
    }
    if let Some(cpus) = spec.limits.cpus {
        args.push("--cpus".into());
        args.push(cpus.into());
    }
    if let Some(pids) = spec.limits.pids {
        args.push("--pids-limit".into());
        args.push(pids.to_string());
    }
    if let Some(size) = spec.limits.storage_opt_size {
        args.push("--storage-opt".into());
        args.push(format!("size={size}"));
    }
    if let Some(entrypoint) = spec.entrypoint {
        args.push("--entrypoint".into());
        args.push(entrypoint.into());
    }
    args.push("--workdir".into());
    args.push(spec.workdir.into());
    for (name, mount) in spec.volumes {
        args.push("--volume".into());
        args.push(format!("{name}:{mount}"));
    }
    for (k, v) in spec.env {
        args.push("--env".into());
        args.push(format!("{k}={v}"));
    }
    args.push(spec.image.into());
    args.extend(spec.argv.iter().map(|s| s.to_string()));
    args
}

/// A docker-managed volume, removed on drop.
pub struct Volume<'a> {
    docker: &'a Docker,
    name: String,
}

impl Volume<'_> {
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for Volume<'_> {
    fn drop(&mut self) {
        let _ = self.docker.run(&["volume", "rm", "--force", &self.name]);
    }
}

/// A created container, removed on drop.
pub struct Container<'a> {
    docker: &'a Docker,
    id: String,
}

/// Result of running a build container to completion.
pub struct RunOutput {
    pub exit_code: i32,
    /// Interleaved stdout+stderr of the build.
    pub log: String,
}

impl Container<'_> {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Copy a tar archive into the container at `dest` (`docker cp - <id>:<dest>`).
    pub fn put_archive(&self, dest: &str, tar: &[u8]) -> Result<()> {
        let mut child = self
            .docker
            .command()
            .args(["cp", "-", &format!("{}:{}", self.id, dest)])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| VerifyError::Docker(format!("`docker cp` (in) failed to start: {e}")))?;

        child
            .stdin
            .take()
            .expect("stdin was piped")
            .write_all(tar)
            .map_err(|e| VerifyError::Docker(format!("writing source tar to `docker cp`: {e}")))?;

        let out = child
            .wait_with_output()
            .map_err(|e| VerifyError::Docker(format!("`docker cp` (in) failed: {e}")))?;
        if !out.status.success() {
            return Err(VerifyError::Docker(format!(
                "`docker cp` (in) failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(())
    }

    /// Copy a path out of the container as a tar archive (`docker cp <id>:<src> -`).
    pub fn get_archive(&self, src: &str) -> Result<Vec<u8>> {
        let out = self
            .docker
            .command()
            .args(["cp", &format!("{}:{}", self.id, src), "-"])
            .output()
            .map_err(|e| VerifyError::Docker(format!("`docker cp` (out) failed: {e}")))?;
        if !out.status.success() {
            return Err(VerifyError::Docker(format!(
                "`docker cp` (out) of `{src}` failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(out.stdout)
    }

    /// Start the container and wait for it, capturing its log.
    ///
    /// On timeout the container is killed and [`VerifyError::Timeout`] returned.
    pub fn run_to_completion(&self, timeout: Duration) -> Result<RunOutput> {
        let mut child = self
            .docker
            .command()
            .args(["start", "--attach", &self.id])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| VerifyError::Docker(format!("`docker start` failed to start: {e}")))?;

        // Drain both pipes from threads: waiting on the child while its pipes
        // fill would deadlock on a chatty build.
        let mut stdout = child.stdout.take().expect("stdout was piped");
        let mut stderr = child.stderr.take().expect("stderr was piped");
        let out_thread = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stdout.read_to_end(&mut buf);
            buf
        });
        let err_thread = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf);
            buf
        });

        let status = child
            .wait_timeout(timeout)
            .map_err(|e| VerifyError::Docker(format!("waiting on build container: {e}")))?;

        let Some(status) = status else {
            // Kill the CLI, then the container it is attached to; otherwise the
            // build keeps burning CPU after we have stopped caring.
            let _ = child.kill();
            let _ = child.wait();
            let _ = self.docker.run(&["kill", &self.id]);
            return Err(VerifyError::Timeout(timeout));
        };

        let mut log = String::from_utf8_lossy(&out_thread.join().unwrap_or_default()).into_owned();
        log.push_str(&String::from_utf8_lossy(
            &err_thread.join().unwrap_or_default(),
        ));

        Ok(RunOutput {
            exit_code: status.code().unwrap_or(-1),
            log,
        })
    }
}

impl Drop for Container<'_> {
    fn drop(&mut self) {
        let _ = self.docker.run(&["rm", "--force", "--volumes", &self.id]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_args_carry_network_isolation_and_resource_caps() {
        let argv = ["contract".to_string(), "build".to_string()];
        let spec = ContainerSpec {
            image: "img@sha256:abc",
            entrypoint: None,
            argv: &argv,
            workdir: "/build/source",
            env: &[("CARGO_HOME", "/cargo-home")],
            volumes: &[("vol", "/cargo-home")],
            network: Network::None,
            limits: ResourceLimits {
                memory: Some("3g"),
                memory_swap: Some("3g"),
                cpus: Some("2"),
                pids: Some(2048),
                storage_opt_size: Some("10g"),
            },
            security: SecurityOpts {
                cap_drop_all: true,
                no_new_privileges: true,
            },
        };
        let args = create_args(&spec);
        let joined = args.join(" ");

        assert_eq!(args.first().map(String::as_str), Some("create"));
        assert!(joined.contains("--network=none"), "{joined}");
        // The caps that keep an untrusted build from exhausting the host.
        assert!(joined.contains("--memory 3g"), "{joined}");
        // --memory-swap == --memory disables swap so the ceiling is not ~2×.
        assert!(joined.contains("--memory-swap 3g"), "{joined}");
        assert!(joined.contains("--cpus 2"), "{joined}");
        assert!(joined.contains("--pids-limit 2048"), "{joined}");
        // Disk quota on the writable layer, when a quota-capable driver allows it.
        assert!(joined.contains("--storage-opt size=10g"), "{joined}");
        // Privilege hardening: no capabilities, no setuid escalation.
        assert!(joined.contains("--cap-drop=ALL"), "{joined}");
        assert!(
            joined.contains("--security-opt=no-new-privileges"),
            "{joined}"
        );
        // Image and its argv come last, image before argv.
        let img = args.iter().position(|s| s == "img@sha256:abc").unwrap();
        let arg = args.iter().position(|s| s == "contract").unwrap();
        assert!(img < arg, "image must precede argv");
    }

    #[test]
    fn create_args_omit_unset_limits_and_default_network() {
        let spec = ContainerSpec {
            image: "img",
            entrypoint: None,
            argv: &[],
            workdir: "/w",
            env: &[],
            volumes: &[],
            network: Network::Bridge,
            limits: ResourceLimits::default(),
            security: SecurityOpts::default(),
        };
        let joined = create_args(&spec).join(" ");
        assert!(!joined.contains("--memory"), "{joined}");
        assert!(!joined.contains("--cpus"), "{joined}");
        assert!(!joined.contains("--pids-limit"), "{joined}");
        // The opt-in caps must add no flags when their fields are unset.
        assert!(!joined.contains("--memory-swap"), "{joined}");
        assert!(!joined.contains("--storage-opt"), "{joined}");
        // Hardening flags are opt-in too: nothing emitted when unset.
        assert!(!joined.contains("--cap-drop"), "{joined}");
        assert!(!joined.contains("--security-opt"), "{joined}");
        // Bridge is the daemon default, so no --network flag is emitted.
        assert!(!joined.contains("--network"), "{joined}");
    }

    #[test]
    fn create_args_emit_a_named_network() {
        // The G4-b seam: the fetch phase can be pinned to a pre-created,
        // egress-filtered network instead of the default bridge.
        let spec = ContainerSpec {
            image: "img",
            entrypoint: None,
            argv: &[],
            workdir: "/w",
            env: &[],
            volumes: &[],
            network: Network::Named("sorofy-fetch"),
            limits: ResourceLimits::default(),
            security: SecurityOpts::default(),
        };
        let joined = create_args(&spec).join(" ");
        assert!(joined.contains("--network=sorofy-fetch"), "{joined}");
        // Not the isolation flag: a named network still has egress.
        assert!(!joined.contains("--network=none"), "{joined}");
    }

    #[test]
    fn create_args_omit_memory_swap_without_memory() {
        // --memory-swap needs --memory to mean anything, so it must not be
        // emitted on its own even if the field is set.
        let spec = ContainerSpec {
            image: "img",
            entrypoint: None,
            argv: &[],
            workdir: "/w",
            env: &[],
            volumes: &[],
            network: Network::Bridge,
            limits: ResourceLimits {
                memory: None,
                memory_swap: Some("3g"),
                ..ResourceLimits::default()
            },
            security: SecurityOpts::default(),
        };
        let joined = create_args(&spec).join(" ");
        assert!(!joined.contains("--memory-swap"), "{joined}");
    }
}
