# Sorofy — API Reference

**Base URL:** `https://sorofy.site` · **Network:** Stellar testnet · **Media type:** `application/json`

Every example below is copy-paste runnable against the live service.

## Reading is public, verifying is not

`GET` needs no credential — a cheap cached lookup is the whole point of the service, and
an explorer or wallet should be able to ask without registering. `POST /verify` spends
build capacity and drives a Docker socket, so it carries a bearer token:

```bash
curl -X POST https://sorofy.site/verify -H "Authorization: Bearer $SOROFY_API_TOKEN" …
```

Without a valid token the request is refused with `401` before its body is even parsed.

---

## `POST /verify` — start a verification

Auth required. Accepts SEP-58 field names.

| Field | Type | Required | Notes |
|---|---|---|---|
| `contract_id` | string | one of `contract_id` / `wasm_hash` | Strkey `C…`. The target hash is resolved **from the network**, never taken from you |
| `wasm_hash` | string | ↑ | Hex sha256. Use when the target is not deployed, or to assert a hash deliberately |
| `repo` | string | one source shape | Git URL — pairs with `rev` |
| `rev` | string | with `repo` | Commit to build |
| `source_uri` | string | one source shape | SEP-58 archive URL — pairs with `source_sha256` |
| `source_sha256` | string | with `source_uri` | Checked **before** anything is unpacked |
| `bldimg` | string | **yes** | SEP-58 build image, digest-pinned (`image@sha256:…`). A bare tag is refused — see below |
| `bldopt` | string[] | no | Flags passed verbatim to `stellar contract build` |

Exactly one source shape — `repo`+`rev` **or** `source_uri`+`source_sha256`. Supplying
both, or neither, is a `400`.

```bash
curl -X POST https://sorofy.site/verify \
  -H "Authorization: Bearer $SOROFY_API_TOKEN" \
  -H 'Content-Type: application/json' -d '{
  "contract_id": "CAZAVVTM3GXFNCLR66FYHJJ43MEEUV3C6PQYRQT5JVGAO2RS6S4OHRT6",
  "repo": "https://github.com/erdemasik001/sorofy-fixture-token",
  "rev": "cd68767f3b36456228b01244ecd4e6f935b5e986",
  "bldimg": "ghcr.io/erdemasik001/sorofy-build-image@sha256:cff44167d2ee90f901c768ccdb75eb5c382c95295f44bcd7ee4b543f0adf9588"
}'
```

`202 Accepted`:

```json
{ "id": 15, "status": "pending", "wasm_hash": "47d2801e115f9a064fe37a8244ef1ffcfa56877668a17f383d3189634a1bcfbd" }
```

The `wasm_hash` in the reply is the answer the *network* gave for that `contract_id`,
resolved during the request. You did not send it and cannot influence it — which is what
stops a submitter choosing the result they want.

### Retroactive shape — source supplied out-of-band

For a contract that carries no SEP-58 metadata on chain, pass the source as an archive.
Note there is still no `wasm_hash`: the target comes from the network.

```bash
curl -X POST https://sorofy.site/verify \
  -H "Authorization: Bearer $SOROFY_API_TOKEN" \
  -H 'Content-Type: application/json' -d '{
  "contract_id": "CAZAVVTM3GXFNCLR66FYHJJ43MEEUV3C6PQYRQT5JVGAO2RS6S4OHRT6",
  "source_uri": "https://github.com/erdemasik001/sorofy-fixture-token/archive/cd68767f3b36456228b01244ecd4e6f935b5e986.tar.gz",
  "source_sha256": "1cde007365bb93f6dae9b6f2e42b0bf29364c44fa031399116a6cfafa4ede416",
  "bldimg": "ghcr.io/erdemasik001/sorofy-build-image@sha256:cff44167d2ee90f901c768ccdb75eb5c382c95295f44bcd7ee4b543f0adf9588"
}'
```

---

## `GET /verify/{key}` — read a result

Public. `key` is a **job id**, a **contract id**, or a **wasm hash** — all three resolve
to the same record, so a caller can ask with whatever identifier it happens to hold. For
a contract id the newest job wins.

```bash
curl https://sorofy.site/verify/CAZAVVTM3GXFNCLR66FYHJJ43MEEUV3C6PQYRQT5JVGAO2RS6S4OHRT6
curl https://sorofy.site/verify/47d2801e115f9a064fe37a8244ef1ffcfa56877668a17f383d3189634a1bcfbd
curl https://sorofy.site/verify/15
```

### Job object

| Field | Type | Notes |
|---|---|---|
| `id` | number | Job number |
| `status` | string | `pending` · `verified` · `mismatch` · `error` |
| `contract_id` | string \| null | Null when the request targeted a bare `wasm_hash` |
| `wasm_hash` | string | The target: resolved on chain, or asserted by the caller |
| `bldimg` | string | Build image as submitted |
| `source` | object | `{kind: "git", repo, rev}` or `{kind: "archive", uri, source_sha256}` |
| `report` | object \| null | Present once the build finishes; null while `pending` |
| `error` | string | Only on `status: "error"` |
| `created_at` / `updated_at` | string | RFC 3339, UTC |

### Report object

| Field | Type | Notes |
|---|---|---|
| `result` | string | `verified` or `mismatch` — mirrors `status` |
| `expected_wasm_sha256` | string | **What the network says.** The claim under test |
| `rebuilt_wasm_sha256` | string | **What we got.** Equal to the above ⇒ `verified` |
| `rebuilt_wasm_size` | number | Bytes. Equal sizes prove nothing; the hash decides |
| `artifact` | string | Which `.wasm` in the build output was selected |
| `bldimg` / `bldimg_digest` | string | As submitted / as resolved by the daemon |
| `bldopt` | string[] | Build flags used |
| `source_sha256` | string | Archive digest, or the staged tree's digest for a git source |
| `trust_level` | string | `arbitrary` today. `publicly-auditable` / `sdf-maintained` arrive with the vetted-image allowlist |
| `build_seconds` | number | Wall clock for the containerised rebuild |
| `build_log` | string | Full compiler output, kept on both outcomes |

> **`trust_level` is not a verdict.** A reproducible build proves "these bytes came from
> this source in this image" — not that the image is honest. `arbitrary` says the image
> was not vetted by us. See [security.md](security.md).

---

## `GET /verifications` — recent results

Public. Newest first.

| Query | Default | Notes |
|---|---|---|
| `limit` | 24 | Clamped to 200 in the database layer, where a handler cannot lift it |
| `offset` | 0 | |

```bash
curl 'https://sorofy.site/verifications?limit=5'
```

```json
{
  "items":  [ /* job objects, newest first */ ],
  "total":  15,
  "limit":  5,
  "offset": 0
}
```

`total` is read after the page, so a job finishing in between can make it slightly stale
— never inconsistent with the rows already returned. It is there so a client can page
without walking off the end.

## `GET /health` — liveness

Public. `200` with a cache ping, `503` if the database does not answer.

```bash
curl https://sorofy.site/health
# {"service":"sorofy","status":"ok"}
```

## `GET /metrics` — job counters

Public. Counters are **process-lifetime** and reset when the service restarts; the cache
does not. Read a baseline before a redeploy, not after.

```bash
curl https://sorofy.site/metrics
# {"jobs_submitted":15,"jobs_in_flight":0,"verified":14,"mismatch":1,"error":0,"avg_build_seconds":86.1}
```

## `GET /` — endpoint listing or explorer

Content-negotiated on `Accept`: a browser asking for `text/html` gets the explorer UI,
anything else (including `curl`'s `*/*`) gets the endpoint listing as JSON.

---

## Status codes

| Code | When |
|---|---|
| `202` | `POST /verify` accepted; the job is queued |
| `200` | Read succeeded |
| `304` | Asset unchanged (`ETag`) |
| `400` | Malformed request: no source shape, both shapes, unparseable `contract_id` |
| `401` | Missing or invalid bearer token on `POST` |
| `404` | `{"error":"not_found"}` — nothing recorded for that key |
| `415` | `POST` without `Content-Type: application/json` *(after auth)* |
| `422` | Body does not deserialise — e.g. a missing required field *(after auth)* |
| `429` | Rate limited, or too many jobs outstanding |
| `503` | `/health` only: the cache is not answering |

Errors are `{"error": "<message>"}`.

## Limits

| Limit | Value | Behaviour at the edge |
|---|---|---|
| Request rate (`POST`) | burst 10, refill 1/s, per principal | `429` + `Retry-After` |
| Outstanding jobs | 16 | `429` |
| Concurrent builds | 2 | Excess queues rather than failing |

The rate limit is keyed on the bearer token, so a leaked token is throttled in aggregate
however many hosts replay it.

## Verification lifecycle

```
POST /verify ──► pending ──► verified   rebuilt hash == on-chain hash
                        ├──► mismatch   rebuilt hash != the target
                        └──► error      fetch, build, or timeout failed
```

`GET` after that is a cached read: the rebuild happens once, and every later query is
served from SQLite. That is what makes this an ecosystem service rather than a local
tool — an explorer can ask on every page view without paying for a build.

### Where each check happens

The `POST` does cheap, local validation — source shape, strkey parsing, the on-chain
lookup — and anything that needs the engine is checked when the job runs. So a request
can be accepted with `202` and still end in `status: "error"`. Digest pinning is the case
worth knowing:

```bash
# bldimg as a bare tag → 202, and then:
curl https://sorofy.site/verify/16
```

```json
{
  "id": 16,
  "status": "error",
  "bldimg": "sorofy/build-image:tag",
  "error": "build image must be digest-pinned (`image@sha256:...`), got `sorofy/build-image:tag`"
}
```

The refusal still lands **before any container is created** — a tag can be moved, so a
tag is not a build environment. It is simply enforced by the engine rather than the
request handler.

---

*Threat model: [security.md](security.md) · deploy procedure:
[deploy-playbook.md](deploy-playbook.md) · non-technical summary:
[delivery-note.md](delivery-note.md)*
