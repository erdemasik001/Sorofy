<#
  Sorofy demo runner — for the README/pitch GIF (PLAN Day3 item 4).

  Two contrasting scenarios against a sorofy-api instance — local, or the live
  testnet service:
    1. Retroactive verify (the money shot): a contract with NO on-chain source
       metadata, verified by supplying its source out-of-band. The target hash is
       resolved on-chain (no wasm_hash is sent).
    2. Tamper -> mismatch: a bogus target hash is claimed; the rebuild refuses to
       match. Shows the check has teeth. Uses the tiny hello-world fixture so the
       build is fast.

  AGAINST THE LIVE SERVICE (roadmap block C3 — the delivery recording):
    # Put the token in the environment, NOT on the command line: this run is
    # being recorded, and the bearer token gates a socket-mounted host.
    $env:SOROFY_API_TOKEN = '<token from /home/sorofy/api-token>'
    powershell -ExecutionPolicy Bypass -File scripts\demo.ps1 -Api https://sorofy.site

  AGAINST A LOCAL INSTANCE:
    # WSL2 Docker reachable + the build image pulled (pre-warm so the recorded
    # build is as fast as possible):
    wsl -d Ubuntu -- docker pull ghcr.io/erdemasik001/sorofy-build-image@sha256:cff44167d2ee90f901c768ccdb75eb5c382c95295f44bcd7ee4b543f0adf9588

    # Start the API in ANOTHER terminal (digest enforcement ON = production mode):
    cargo run -p api --bin sorofy-api

    powershell -ExecutionPolicy Bypass -File scripts\demo.ps1

  Either way: run it once as a dry run to warm cargo/registry caches, and a
  second time while recording.

  WHY ONLY THE POSTs CARRY THE TOKEN
    `POST /verify` spends build capacity and drives the Docker socket, so it is
    gated; `GET` is deliberately public — a cheap cached lookup is the whole
    point of the service. The demo reads results with a bare GET on purpose.
#>

#requires -Version 5
[CmdletBinding()]
param(
    # Base URL of the sorofy-api instance to drive.
    [string]$Api = 'http://localhost:8080',

    # Bearer token for POST /verify. Defaults to the environment so the
    # credential never appears in the recorded terminal or in shell history.
    # Leave unset for a local instance started without SOROFY_API_TOKEN.
    [string]$Token = $env:SOROFY_API_TOKEN,

    # Build wait ceiling. A cold cargo cache on a small host can outrun the
    # default; pre-warming (above) is the better fix, this is the escape hatch.
    [int]$TimeoutSec = 240
)

$ErrorActionPreference = 'Stop'

# Windows PowerShell 5.1 negotiates TLS 1.0/1.1 by default and a modern proxy
# (the live service terminates TLS at Caddy) refuses that, which surfaces as an
# opaque "could not create SSL/TLS secure channel". Add TLS 1.2 rather than
# assigning it, so PowerShell 7's TLS 1.3 is not downgraded on the way past.
if ($PSVersionTable.PSVersion.Major -lt 6) {
    [Net.ServicePointManager]::SecurityProtocol =
        [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
}

# A trailing slash would turn every "$Api/verify" into a double-slashed path.
$Api    = $Api.TrimEnd('/')
$IsLocal = $Api -match '(?i)//(localhost|127\.0\.0\.1|\[::1\])(:|/|$)'
$Bldimg = 'ghcr.io/erdemasik001/sorofy-build-image@sha256:cff44167d2ee90f901c768ccdb75eb5c382c95295f44bcd7ee4b543f0adf9588'

# POST is token-gated; GET is not. Keeping them separate is what lets the demo
# show a public read against the same live service.
$PostHeaders = @{}
if ($Token) { $PostHeaders['Authorization'] = "Bearer $Token" }

# --- tiny presentation helpers -------------------------------------------------
function Rule([string]$c = 'DarkGray') { Write-Host ('-' * 74) -ForegroundColor $c }
function Head([string]$t) {
    Write-Host ''
    Write-Host ('=' * 74) -ForegroundColor Cyan
    Write-Host "  $t" -ForegroundColor Cyan
    Write-Host ('=' * 74) -ForegroundColor Cyan
}
function KV([string]$k, $v, [string]$c = 'Gray') {
    Write-Host ("  {0,-22}: " -f $k) -NoNewline -ForegroundColor DarkGray
    Write-Host $v -ForegroundColor $c
}

# Poll GET /verify/{id} until the job leaves 'pending'. Prints a status line each
# tick so the build wait is visible progress, not dead air (matters for the GIF).
function Wait-Job([int]$id, [int]$timeoutSec = $script:TimeoutSec) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($true) {
        $row = Invoke-RestMethod -Uri "$Api/verify/$id"
        if ($row.status -ne 'pending') { return $row }
        Write-Host ("  [{0,4:n0}s] building ... status=pending" -f $sw.Elapsed.TotalSeconds) -ForegroundColor DarkYellow
        if ($sw.Elapsed.TotalSeconds -gt $timeoutSec) { throw "timed out after ${timeoutSec}s" }
        Start-Sleep -Seconds 3
    }
}

function Show-Result($row) {
    $rep = $row.report
    if ($null -eq $rep) {
        KV 'status' $row.status 'Red'
        if ($row.error) { KV 'error' $row.error 'Red' }
        return
    }
    Write-Host ''
    KV 'expected (on-chain)'  $rep.expected_wasm_sha256 'White'
    KV 'rebuilt  (container)' $rep.rebuilt_wasm_sha256   'White'
    KV 'rebuilt size'         ("{0} bytes" -f $rep.rebuilt_wasm_size)
    KV 'bldimg digest'        $rep.bldimg_digest
    KV 'trust_level'          $rep.trust_level
    KV 'build seconds'        ("{0:n1}" -f $rep.build_seconds)
    Write-Host ''
    switch ($row.status) {
        'verified' {
            Write-Host '   ####  VERIFIED  ####   rebuilt == on-chain hash' -ForegroundColor Green
        }
        'mismatch' {
            Write-Host '   ####  MISMATCH  ####   rebuilt != claimed hash' -ForegroundColor Red
        }
        default {
            Write-Host ("   ####  {0}  ####" -f $row.status.ToUpper()) -ForegroundColor Yellow
        }
    }
}

# POST /verify, with the 401 that a token-gated instance returns turned into an
# actionable message instead of a raw PowerShell exception mid-recording.
function Invoke-Verify([string]$body) {
    try {
        return Invoke-RestMethod -Method Post -Uri "$Api/verify" `
            -ContentType 'application/json' -Body $body -Headers $PostHeaders
    }
    catch {
        $code = $null
        if ($_.Exception.Response) { $code = [int]$_.Exception.Response.StatusCode }
        if ($code -eq 401) {
            Write-Host ''
            Write-Host '  401 - POST /verify is token-gated on this instance.' -ForegroundColor Red
            Write-Host '  Set the token in the environment (not on the command line - this is recorded):' -ForegroundColor Yellow
            Write-Host '      $env:SOROFY_API_TOKEN = "<token>"' -ForegroundColor White
            exit 1
        }
        throw
    }
}

# --- preflight: server must be up ---------------------------------------------
try {
    Invoke-RestMethod -Uri "$Api/health" -TimeoutSec 10 | Out-Null
}
catch {
    Write-Host "sorofy-api is not answering on $Api" -ForegroundColor Red
    if ($IsLocal) {
        Write-Host "Start it first in another terminal:" -ForegroundColor Yellow
        Write-Host "    cargo run -p api --bin sorofy-api" -ForegroundColor White
    }
    else {
        Write-Host "Check that the host is up, DNS resolves, and TLS is served:" -ForegroundColor Yellow
        Write-Host "    curl $Api/health" -ForegroundColor White
    }
    exit 1
}

Write-Host ''
Write-Host '  Sorofy - Soroban Contract Verification - live demo' -ForegroundColor Cyan
Write-Host '  digest-pinned bldimg, enforcement ON (production mode)' -ForegroundColor DarkGray
# Showing the target is part of the evidence: it is what distinguishes a
# recording against the live testnet service from one against localhost.
KV 'service' $Api 'White'
if (-not $IsLocal -and -not $Token) {
    Write-Host '  warning: no token set; a live instance will reject POST /verify with 401' -ForegroundColor DarkYellow
}

# ============================ SCENARIO 1 ======================================
Head 'SCENARIO 1  -  Retroactive verify (contract carries NO source metadata)'
Write-Host '  The contract stores no bldimg/source_uri on-chain. We supply the source' -ForegroundColor Gray
Write-Host '  out-of-band as an archive; the target hash is resolved FROM the network.' -ForegroundColor Gray
Write-Host ''

$body1 = [ordered]@{
    contract_id   = 'CAZAVVTM3GXFNCLR66FYHJJ43MEEUV3C6PQYRQT5JVGAO2RS6S4OHRT6'
    source_uri    = 'https://github.com/erdemasik001/sorofy-fixture-token/archive/cd68767f3b36456228b01244ecd4e6f935b5e986.tar.gz'
    source_sha256 = '1cde007365bb93f6dae9b6f2e42b0bf29364c44fa031399116a6cfafa4ede416'
    bldimg        = $Bldimg
} | ConvertTo-Json

Write-Host "  POST $Api/verify" -ForegroundColor White
Write-Host $body1 -ForegroundColor DarkGray
Rule
$post1 = Invoke-Verify $body1
KV 'job id'   $post1.id 'Yellow'
KV 'status'   $post1.status 'Yellow'
KV 'wasm_hash (from RPC)' $post1.wasm_hash 'Yellow'
Write-Host '  ^ no wasm_hash was sent - it was resolved on-chain via getLedgerEntries' -ForegroundColor DarkGray
Write-Host ''
$r1 = Wait-Job $post1.id
Show-Result $r1

# ============================ SCENARIO 2 ======================================
Head 'SCENARIO 2  -  Tamper -> mismatch (the check has teeth)'
Write-Host '  Same engine, but the caller claims a bogus target hash (1111...).' -ForegroundColor Gray
Write-Host '  The honest rebuild lands on different bytes -> mismatch, build log kept.' -ForegroundColor Gray
Write-Host '  Uses the tiny hello-world fixture so the build is fast.' -ForegroundColor Gray
Write-Host ''

$body2 = [ordered]@{
    repo      = 'https://github.com/erdemasik001/stellar-verify-fixture-hello-world'
    rev       = 'c08333e9924bfb45ee221f3edeb8ded4d4840397'
    wasm_hash = '1111111111111111111111111111111111111111111111111111111111111111'
    bldimg    = $Bldimg
} | ConvertTo-Json

Write-Host "  POST $Api/verify" -ForegroundColor White
Write-Host $body2 -ForegroundColor DarkGray
Rule
$post2 = Invoke-Verify $body2
KV 'job id' $post2.id 'Yellow'
Write-Host ''
$r2 = Wait-Job $post2.id
Show-Result $r2

Write-Host ''
Rule 'Cyan'
Write-Host '  Retroactive verification proven + tamper rejected. That is the MVP claim.' -ForegroundColor Cyan
Rule 'Cyan'
Write-Host ''
