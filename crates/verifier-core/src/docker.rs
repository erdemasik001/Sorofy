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
        Docker { program: "docker".into(), prefix: vec![] }
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
                return Docker { program, prefix: parts.collect() };
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
        let out = self
            .command()
            .args(args)
            .output()
            .map_err(|e| VerifyError::Docker(format!("could not run `{}`: {e}", self.program)))?;
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
        self.run(&["version", "--format", "{{.Server.Version}}"]).map(|_| ())
    }

    /// Resolve an image's `repo@sha256:...` digest, pulling it if absent.
    ///
    /// This is how a `bldimg` is minted for an image we build locally, and how
    /// we confirm a submitted digest actually resolves.
    pub fn image_digest(&self, image: &str) -> Result<Option<String>> {
        let raw = self.run(&["image", "inspect", image, "--format", "{{json .RepoDigests}}"])?;
        let digests: Vec<String> = serde_json::from_slice(&raw)
            .map_err(|e| VerifyError::Docker(format!("could not parse RepoDigests: {e}")))?;
        Ok(digests.into_iter().next())
    }

    /// Create a stopped container. `argv` is passed to the image's entrypoint.
    pub fn create(&self, spec: &ContainerSpec<'_>) -> Result<Container<'_>> {
        let mut args: Vec<String> = vec!["create".into()];
        if spec.network == Network::None {
            args.push("--network=none".into());
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

        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = self.run(&refs)?;
        let id = String::from_utf8_lossy(&out).trim().to_string();
        if id.is_empty() {
            return Err(VerifyError::Docker("`docker create` returned no container id".into()));
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
        Ok(Volume { docker: self, name: name.to_string() })
    }
}

/// Whether a container can reach the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    /// Default bridge networking.
    Bridge,
    /// `--network=none`.
    None,
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
    pub network: Network,
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
        log.push_str(&String::from_utf8_lossy(&err_thread.join().unwrap_or_default()));

        Ok(RunOutput { exit_code: status.code().unwrap_or(-1), log })
    }
}

impl Drop for Container<'_> {
    fn drop(&mut self) {
        let _ = self.docker.run(&["rm", "--force", "--volumes", &self.id]);
    }
}
