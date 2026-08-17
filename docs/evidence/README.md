# Evidence — Instawards SOW

Captures of the funded deliverables running against the live service and the live
host. Each one is reproducible; the commands that produced them are below, so a
reviewer can re-run any of them rather than take the image on trust.

| File | Deliverable | What it shows |
|---|---|---|
| `a1-verify-core-verified.png` | 1 — build engine | `verify-core` rebuilds the fixture and matches the target hash: `VERIFIED`, exit 0 |
| `a1-verify-core-mismatch.png` | 1 — build engine | The same engine on **one altered word** of source: `MISMATCH`, exit 1 |
| `c1-get-verify-by-contract-id.png` | 2 — public API | `GET /verify/{contract_id}` for a real testnet contract, returning `status` and `trust_level` |
| `c1-get-verify-by-job-id.png` | 2 — public API | The same record fetched by job id — the endpoint accepts either key |
| `c1-explorer-verified.png` | 2 — public API | The same result in a browser, for a non-technical reader |
| `c3-demo.mp4` | 3 — retroactive path | Retroactive verify against a metadata-less contract, then tamper → `mismatch` |

## Deliverable 1 — the engine (`a1-*`)

Both runs are on the live host (`/opt/Sorofy`), against the digest-pinned build
image, with **no** `--allow-unpinned-image`: the same enforcement the deployed
service runs under.

**Verified** — the published fixture at its published commit:

```bash
./target/release/verify-core \
  --repo https://github.com/erdemasik001/stellar-verify-fixture-hello-world \
  --rev c08333e9924bfb45ee221f3edeb8ded4d4840397 \
  --bldimg ghcr.io/erdemasik001/sorofy-build-image@sha256:cff44167d2ee90f901c768ccdb75eb5c382c95295f44bcd7ee4b543f0adf9588 \
  --wasm-hash b68602842d3a1d169d54fe3e57c0511a774df4710553d6d4d22e653d62bf5f5b
```

**Mismatch** — `/tmp/tampered` in that screenshot is a clone of the *same fixture
at the same commit* with exactly one word changed in
`contracts/hello-world/src/lib.rs`, committed so the engine builds it as a real
revision (it archives a commit, never a dirty working tree):

```bash
git clone https://github.com/erdemasik001/stellar-verify-fixture-hello-world /tmp/tampered
cd /tmp/tampered && git checkout c08333e9924bfb45ee221f3edeb8ded4d4840397 -b tamper
sed -i 's/"Hello"/"Howdy"/' contracts/hello-world/src/lib.rs
git commit -am 'tamper: Hello -> Howdy'
```

The point of the pair is what the two images share and what they do not. The
altered contract compiles to a file of **exactly the same size**, 660 bytes, so
size gives nothing away — but the hash is unrelated (`2f8a8fff…` against the
expected `b68602…`) and the engine returns `MISMATCH` with exit code 1. A
verifier that cannot fail is worth nothing; this is the run where it fails.

`2f8a8fff…` is also the value recorded in
[day1-build-engine.md](../day1-build-engine.md) days earlier on a different
machine — so the pair doubles as a determinism check, not just a sensitivity one.

## Deliverable 2 — the API (`c1-*`)

```bash
curl -s https://sorofy.site/verify/CAZAVVTM3GXFNCLR66FYHJJ43MEEUV3C6PQYRQT5JVGAO2RS6S4OHRT6 \
  | python3 -m json.tool
```

`CAZAVVTM…` is a **real testnet contract**
([StellarExpert](https://stellar.expert/explorer/testnet/contract/CAZAVVTM3GXFNCLR66FYHJJ43MEEUV3C6PQYRQT5JVGAO2RS6S4OHRT6))
whose target hash was resolved from the network over RPC, not supplied by the
caller. The second shot fetches the same record by job id instead — the endpoint
takes either key, and a caller who has just submitted a job usually holds the id
rather than the contract. The browser shot is that record rendered for a reader
who would rather not read JSON:
<https://sorofy.site/#/v/CAZAVVTM3GXFNCLR66FYHJJ43MEEUV3C6PQYRQT5JVGAO2RS6S4OHRT6>

Both are live: the numbers can be recomputed at any time from
<https://sorofy.site/verifications>.

## Deliverable 3 — the retroactive path (`c3-demo`)

```bash
export SOROFY_API_TOKEN='<token>'
./scripts/demo.sh --api https://sorofy.site
```

Two scenarios back to back: a contract carrying **no** SEP-58 source metadata
verified from source supplied out-of-band (the hash still resolved on-chain), and
then a claimed-but-wrong target hash rejected as `mismatch`.
