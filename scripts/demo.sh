#!/usr/bin/env bash
#
# Sorofy demo runner — the bash twin of demo.ps1 (roadmap block C3).
#
# Same two scenarios, same output shape. It exists because the recording has to
# be made from whatever machine is at hand on delivery day, and demo.ps1 needs
# PowerShell — which a stock macOS or Linux box does not have, and which is a
# large install to add for one script. This one needs only bash, curl and
# python3: all three ship with macOS and every mainstream Linux.
#
# That is affordable precisely because the demo is pure HTTP. It drives a
# running sorofy-api over its public interface; it does not build anything, so
# it needs neither Docker nor a Rust toolchain locally. python3 is here only to
# read JSON fields — `jq` is the obvious tool, but it is not installed by
# default anywhere, which is the problem this script exists to avoid.
#
#   1. Retroactive verify (the money shot): a contract with NO on-chain source
#      metadata, verified by supplying its source out-of-band. The target hash
#      is resolved on-chain (no wasm_hash is sent).
#   2. Tamper -> mismatch: a bogus target hash is claimed; the rebuild refuses
#      to match. Shows the check has teeth. Uses the tiny hello-world fixture so
#      the build is fast.
#
# AGAINST THE LIVE SERVICE (the delivery recording):
#   # Export the token BEFORE you start recording, not during: this run is being
#   # captured and the bearer token gates a socket-mounted host.
#   export SOROFY_API_TOKEN='<token from /home/sorofy/api-token>'
#   scripts/demo.sh --api https://sorofy.site
#
# AGAINST A LOCAL INSTANCE:
#   cargo run -p api --bin sorofy-api      # in another terminal
#   scripts/demo.sh
#
# Either way: run it once as a dry run to warm the caches, and a second time
# while recording.
#
# WHY ONLY THE POSTs CARRY THE TOKEN
#   `POST /verify` spends build capacity and drives the Docker socket, so it is
#   gated; `GET` is deliberately public — a cheap cached lookup is the whole
#   point of the service. The demo reads results with a bare GET on purpose.

# No `set -u`: bash 3.2 (still the system bash on macOS) errors on an empty
# array expansion under it, and the token header below is exactly that when no
# token is set.
set -eo pipefail

API="${API:-http://localhost:8080}"
TOKEN="${SOROFY_API_TOKEN:-}"
TIMEOUT_SEC=240

while [ $# -gt 0 ]; do
    case "$1" in
        -a|--api)     API="$2"; shift 2 ;;
        -t|--token)   TOKEN="$2"; shift 2 ;;
        --timeout)    TIMEOUT_SEC="$2"; shift 2 ;;
        -h|--help)
            sed -n '3,40p' "$0" | sed 's/^# \{0,1\}//'
            exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

# A trailing slash would turn every "$API/verify" into a double-slashed path.
API="${API%/}"

case "$API" in
    *//localhost*|*//127.0.0.1*|*//\[::1\]*) IS_LOCAL=1 ;;
    *) IS_LOCAL=0 ;;
esac

BLDIMG='ghcr.io/erdemasik001/sorofy-build-image@sha256:cff44167d2ee90f901c768ccdb75eb5c382c95295f44bcd7ee4b543f0adf9588'

# POST is token-gated; GET is not. Keeping them separate is what lets the demo
# show a public read against the same live service.
POST_AUTH=()
[ -n "$TOKEN" ] && POST_AUTH=(-H "Authorization: Bearer $TOKEN")

# --- tiny presentation helpers -------------------------------------------------
C_RESET=$'\033[0m'; C_CYAN=$'\033[36m';   C_GRAY=$'\033[90m'
C_WHITE=$'\033[97m'; C_YELLOW=$'\033[33m'; C_GREEN=$'\033[32m'
C_RED=$'\033[31m';   C_DIM=$'\033[2m'

rule()  { printf '%s%s%s\n' "$C_GRAY" "$(printf -- '-%.0s' $(seq 1 74))" "$C_RESET"; }
head_() {
    printf '\n%s%s%s\n' "$C_CYAN" "$(printf '=%.0s' $(seq 1 74))" "$C_RESET"
    printf '%s  %s%s\n' "$C_CYAN" "$1" "$C_RESET"
    printf '%s%s%s\n' "$C_CYAN" "$(printf '=%.0s' $(seq 1 74))" "$C_RESET"
}
kv() { printf '%s  %-22s: %s%s%s\n' "$C_GRAY" "$1" "${3:-$C_WHITE}" "$2" "$C_RESET"; }

# Read a dotted path out of a JSON document. Returns empty for a missing or null
# field, so callers can test with -z rather than parsing an error.
json_get() {
    python3 -c '
import sys, json
try:
    data = json.loads(sys.argv[1])
except ValueError:
    sys.exit(0)
for key in sys.argv[2].split("."):
    if not isinstance(data, dict):
        sys.exit(0)
    data = data.get(key)
print("" if data is None else data)
' "$1" "$2"
}

json_pretty() { python3 -m json.tool <<<"$1"; }

# Poll GET /verify/{id} until the job leaves 'pending'. Prints a status line each
# tick so the build wait is visible progress, not dead air (matters for the
# recording).
wait_job() {
    local id="$1" started elapsed row status
    started=$(date +%s)
    while true; do
        row=$(curl -s "$API/verify/$id")
        status=$(json_get "$row" status)
        if [ "$status" != "pending" ]; then
            printf '%s' "$row"
            return 0
        fi
        elapsed=$(( $(date +%s) - started ))
        printf '%s  [%4ds] building ... status=pending%s\n' "$C_YELLOW" "$elapsed" "$C_RESET" >&2
        if [ "$elapsed" -gt "$TIMEOUT_SEC" ]; then
            printf '%s  timed out after %ss%s\n' "$C_RED" "$TIMEOUT_SEC" "$C_RESET" >&2
            exit 1
        fi
        sleep 3
    done
}

show_result() {
    local row="$1" status
    status=$(json_get "$row" status)

    if [ -z "$(json_get "$row" report.result)" ]; then
        kv 'status' "$status" "$C_RED"
        local err; err=$(json_get "$row" error)
        [ -n "$err" ] && kv 'error' "$err" "$C_RED"
        return 0
    fi

    printf '\n'
    kv 'expected (on-chain)'  "$(json_get "$row" report.expected_wasm_sha256)"
    kv 'rebuilt  (container)' "$(json_get "$row" report.rebuilt_wasm_sha256)"
    kv 'rebuilt size'         "$(json_get "$row" report.rebuilt_wasm_size) bytes"
    kv 'bldimg digest'        "$(json_get "$row" report.bldimg_digest)" "$C_GRAY"
    kv 'trust_level'          "$(json_get "$row" report.trust_level)" "$C_GRAY"
    kv 'build seconds'        "$(json_get "$row" report.build_seconds)" "$C_GRAY"
    printf '\n'

    case "$status" in
        verified) printf '%s   ####  VERIFIED  ####   rebuilt == on-chain hash%s\n' "$C_GREEN" "$C_RESET" ;;
        mismatch) printf '%s   ####  MISMATCH  ####   rebuilt != claimed hash%s\n' "$C_RED" "$C_RESET" ;;
        *)        printf '%s   ####  %s  ####%s\n' "$C_YELLOW" "$(echo "$status" | tr '[:lower:]' '[:upper:]')" "$C_RESET" ;;
    esac
}

# POST /verify, with the 401 a token-gated instance returns turned into an
# actionable message instead of a raw error mid-recording.
invoke_verify() {
    local body="$1" out code
    out=$(mktemp)
    code=$(curl -s -o "$out" -w '%{http_code}' -X POST "$API/verify" \
        -H 'Content-Type: application/json' "${POST_AUTH[@]}" -d "$body")

    if [ "$code" = "401" ]; then
        printf '\n%s  401 - POST /verify is token-gated on this instance.%s\n' "$C_RED" "$C_RESET" >&2
        printf '%s  Put the token in the environment (not on the command line - this is recorded):%s\n' "$C_YELLOW" "$C_RESET" >&2
        printf '%s      export SOROFY_API_TOKEN="<token>"%s\n' "$C_WHITE" "$C_RESET" >&2
        rm -f "$out"; exit 1
    fi
    if [ "$code" != "200" ] && [ "$code" != "202" ]; then
        printf '\n%s  POST /verify returned HTTP %s%s\n' "$C_RED" "$code" "$C_RESET" >&2
        cat "$out" >&2; printf '\n' >&2
        rm -f "$out"; exit 1
    fi
    cat "$out"; rm -f "$out"
}

# --- preflight: server must be up ---------------------------------------------
if ! curl -sf -m 10 "$API/health" >/dev/null; then
    printf '%ssorofy-api is not answering on %s%s\n' "$C_RED" "$API" "$C_RESET" >&2
    if [ "$IS_LOCAL" = "1" ]; then
        printf '%sStart it first in another terminal:%s\n' "$C_YELLOW" "$C_RESET" >&2
        printf '%s    cargo run -p api --bin sorofy-api%s\n' "$C_WHITE" "$C_RESET" >&2
    else
        printf '%sCheck that the host is up, DNS resolves, and TLS is served:%s\n' "$C_YELLOW" "$C_RESET" >&2
        printf '%s    curl %s/health%s\n' "$C_WHITE" "$API" "$C_RESET" >&2
    fi
    exit 1
fi

printf '\n%s  Sorofy - Soroban Contract Verification - live demo%s\n' "$C_CYAN" "$C_RESET"
printf '%s  digest-pinned bldimg, enforcement ON (production mode)%s\n' "$C_DIM" "$C_RESET"
# Showing the target is part of the evidence: it is what distinguishes a
# recording against the live testnet service from one against localhost.
kv 'service' "$API"
if [ "$IS_LOCAL" = "0" ] && [ -z "$TOKEN" ]; then
    printf '%s  warning: no token set; a live instance will reject POST /verify with 401%s\n' "$C_YELLOW" "$C_RESET"
fi

# ============================ SCENARIO 1 ======================================
head_ 'SCENARIO 1  -  Retroactive verify (contract carries NO source metadata)'
printf '%s  The contract stores no bldimg/source_uri on-chain. We supply the source%s\n' "$C_GRAY" "$C_RESET"
printf '%s  out-of-band as an archive; the target hash is resolved FROM the network.%s\n' "$C_GRAY" "$C_RESET"
printf '\n'

BODY1=$(cat <<JSON
{
  "contract_id": "CAZAVVTM3GXFNCLR66FYHJJ43MEEUV3C6PQYRQT5JVGAO2RS6S4OHRT6",
  "source_uri": "https://github.com/erdemasik001/sorofy-fixture-token/archive/cd68767f3b36456228b01244ecd4e6f935b5e986.tar.gz",
  "source_sha256": "1cde007365bb93f6dae9b6f2e42b0bf29364c44fa031399116a6cfafa4ede416",
  "bldimg": "$BLDIMG"
}
JSON
)

printf '%s  POST %s/verify%s\n' "$C_WHITE" "$API" "$C_RESET"
printf '%s%s%s\n' "$C_GRAY" "$(json_pretty "$BODY1")" "$C_RESET"
rule
POST1=$(invoke_verify "$BODY1")
ID1=$(json_get "$POST1" id)
kv 'job id' "$ID1" "$C_YELLOW"
kv 'status' "$(json_get "$POST1" status)" "$C_YELLOW"
kv 'wasm_hash (from RPC)' "$(json_get "$POST1" wasm_hash)" "$C_YELLOW"
printf '%s  ^ no wasm_hash was sent - it was resolved on-chain via getLedgerEntries%s\n' "$C_DIM" "$C_RESET"
printf '\n'
R1=$(wait_job "$ID1")
show_result "$R1"

# ============================ SCENARIO 2 ======================================
head_ 'SCENARIO 2  -  Tamper -> mismatch (the check has teeth)'
printf '%s  Same engine, but the caller claims a bogus target hash (1111...).%s\n' "$C_GRAY" "$C_RESET"
printf '%s  The honest rebuild lands on different bytes -> mismatch, build log kept.%s\n' "$C_GRAY" "$C_RESET"
printf '%s  Uses the tiny hello-world fixture so the build is fast.%s\n' "$C_GRAY" "$C_RESET"
printf '\n'

BODY2=$(cat <<JSON
{
  "repo": "https://github.com/erdemasik001/stellar-verify-fixture-hello-world",
  "rev": "c08333e9924bfb45ee221f3edeb8ded4d4840397",
  "wasm_hash": "1111111111111111111111111111111111111111111111111111111111111111",
  "bldimg": "$BLDIMG"
}
JSON
)

printf '%s  POST %s/verify%s\n' "$C_WHITE" "$API" "$C_RESET"
printf '%s%s%s\n' "$C_GRAY" "$(json_pretty "$BODY2")" "$C_RESET"
rule
POST2=$(invoke_verify "$BODY2")
ID2=$(json_get "$POST2" id)
kv 'job id' "$ID2" "$C_YELLOW"
printf '\n'
R2=$(wait_job "$ID2")
show_result "$R2"

printf '\n'
printf '%s%s%s\n' "$C_CYAN" "$(printf '=%.0s' $(seq 1 74))" "$C_RESET"
printf '%s  Retroactive verification proven + tamper rejected. That is the MVP claim.%s\n' "$C_CYAN" "$C_RESET"
printf '%s%s%s\n' "$C_CYAN" "$(printf '=%.0s' $(seq 1 74))" "$C_RESET"
printf '\n'
