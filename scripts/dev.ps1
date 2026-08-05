<#
  Sorofy dev launcher — starts sorofy-api (and with it the explorer UI) for
  local work on this Windows box.

  WHY IT GOES THROUGH WSL
    The service does a Docker preflight at startup and refuses to boot without a
    reachable daemon, because it spawns build containers. Docker Desktop is
    broken on this machine; Docker Engine runs natively inside WSL2, so the API
    has to run there too. The bind address is 0.0.0.0 for the same reason: WSL2
    only forwards a port to Windows localhost if the listener is not bound to
    127.0.0.1.

  USAGE
    powershell -ExecutionPolicy Bypass -File scripts\dev.ps1
    powershell -ExecutionPolicy Bypass -File scripts\dev.ps1 -Port 9000
    powershell -ExecutionPolicy Bypass -File scripts\dev.ps1 -DbPath /mnt/d/sorofy.db
    powershell -ExecutionPolicy Bypass -File scripts\dev.ps1 -Stop

  NOTES
    - Ctrl-C stops it cleanly: the service handles SIGTERM/Ctrl-C, drains
      in-flight requests, and reconciles jobs a previous run left mid-flight.
    - The UI assets (index.html / app.css / app.js / fonts) are compiled into
      the binary with include_str!/include_bytes!, so editing them does nothing
      until the next start — this script always rebuilds, which is the point.
    - The cargo target dir is kept inside WSL rather than the repo's target/:
      it avoids fighting the Windows build, and building off /mnt is slow.
#>

#requires -Version 5
[CmdletBinding()]
param(
    [int]    $Port   = 8091,
    # Not -Db: that collides with the -Debug common parameter's `db` alias.
    [string] $DbPath = '/tmp/sorofy.db',
    [string] $Distro = 'Ubuntu',
    [string] $Token,
    [switch] $Release,
    [switch] $Stop
)

$ErrorActionPreference = 'Stop'

function Info([string]$m) { Write-Host "  $m" -ForegroundColor Gray }
function Ok  ([string]$m) { Write-Host "  $m" -ForegroundColor Green }
function Die ([string]$m) { Write-Host ''; Write-Host "  $m" -ForegroundColor Red; exit 1 }

Write-Host ''
Write-Host '  sorofy-api — dev launcher' -ForegroundColor Cyan
Write-Host ('  ' + '-' * 40) -ForegroundColor DarkGray

# --- preflight ---------------------------------------------------------------
# Each check exists so a failure names its own cause instead of surfacing as a
# confusing error from three layers down.

if (-not (Get-Command wsl.exe -ErrorAction SilentlyContinue)) {
    Die 'wsl.exe not found. This project builds and runs inside WSL2 (see the header).'
}

$distros = & wsl.exe --list --quiet 2>$null
if ($LASTEXITCODE -ne 0) { Die 'Could not list WSL distributions. Is WSL installed and running?' }
# `wsl --list` emits UTF-16, which survives as NUL bytes once PowerShell has it.
$distroList = ($distros -join ' ') -replace "`0", ''
if ($distroList -notmatch [regex]::Escape($Distro)) {
    Die "WSL distribution '$Distro' not found. Available: $($distroList.Trim()). Pass -Distro <name>."
}

# Every WSL call goes through `bash -lc` with its redirections written in bash.
# PowerShell 5.1 wraps a native command's stderr in a NativeCommandError, which
# under $ErrorActionPreference='Stop' kills the script over output that is not
# even an error — so stderr is dealt with on the Linux side, never here.
function Wsl([string]$script) {
    & wsl.exe -d $Distro -- bash -lc $script
}

if ($Stop) {
    Wsl 'pkill -f sorofy-api >/dev/null 2>&1 || true' | Out-Null
    Ok 'Stopped any running sorofy-api in WSL.'
    exit 0
}

$dockerVersion = (Wsl "docker version --format '{{.Server.Version}}' 2>/dev/null" | Out-String).Trim()
if (-not $dockerVersion) {
    Die "Docker is not reachable inside '$Distro'. The API refuses to start without it. Try: wsl -d $Distro -- sudo service docker start"
}

# Derive the repo root from this script rather than hardcoding it, and let WSL
# translate it — wslpath is the only reliable Windows->/mnt mapping, including on
# a drive that is not C:. Backslashes are swapped for forward slashes first:
# passed as-is they are consumed before wslpath ever sees them, which turns
# `D:\projeler\x` into `D:projelerx`.
$repoRoot = (Split-Path -Parent $PSScriptRoot) -replace '\\', '/'
$wslRepo  = (Wsl "wslpath -a '$repoRoot' 2>/dev/null" | Out-String).Trim()
if (-not $wslRepo) { Die "Could not translate '$repoRoot' to a WSL path." }

# A stale instance holds the port and the new one would fail to bind, so clear
# it first. This is a dev launcher; there is nothing here worth preserving.
Wsl 'pkill -f sorofy-api >/dev/null 2>&1 || true' | Out-Null

# The service runs inside WSL, so the database path has to be a WSL path. A
# Windows one is the natural thing to type from here, so translate it rather
# than failing later with sqlite's "unable to open database file".
if ($DbPath -match '^[A-Za-z]:[\\/]') {
    $winDb   = $DbPath -replace '\\', '/'
    $DbPath  = (Wsl "wslpath -a '$winDb' 2>/dev/null" | Out-String).Trim()
    if (-not $DbPath) { Die "Could not translate the database path '$winDb' to a WSL path." }
}

$profileArg = if ($Release) { '--release ' } else { '' }
# `$HOME` is escaped so bash expands it, not PowerShell. Not named $env: that
# is PowerShell's environment drive and reads as a typo here.
$envVars = "CARGO_TARGET_DIR=`$HOME/sorofy-target SOROFY_DB='$DbPath' SOROFY_BIND=0.0.0.0:$Port"
if ($Token) { $envVars += " SOROFY_API_TOKEN='$Token'" }

Info "repo    $wslRepo"
Info "db      $DbPath"
Info "profile $(if ($Release) { 'release' } else { 'debug' })"
Info "auth    $(if ($Token) { 'bearer token set' } else { 'open (POST /verify unauthenticated)' })"
Write-Host ''
Ok   "http://localhost:$Port"
Info 'Ctrl-C to stop. First run compiles, so give it a moment.'
Write-Host ''

# cargo writes its progress ("Compiling…", "Finished…", "Running…") to stderr,
# and so does the service's own tracing output. PowerShell 5.1 turns a native
# command's stderr into error records, which under 'Stop' terminates the script
# and takes the server down with it — observed: the API started, logged one line,
# and died. None of that output is a failure, so the preference is relaxed for
# the run itself. Everything above this point still fails fast.
$ErrorActionPreference = 'Continue'

# A login shell, because a plain non-interactive one does not have cargo on PATH.
Wsl "cd '$wslRepo' && $envVars cargo run ${profileArg}-p api --bin sorofy-api"
