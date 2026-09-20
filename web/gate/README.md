# Verification gate — anchor deposit, gated on consensus

> Implements the **front-end gate** of [docs/anchor-integration.md](../../docs/anchor-integration.md)
> §4a, plus the anchor on-ramp of §5. The on-chain gate (§4b) is a separate spike and is not
> here. Testnet only.

A user brings a balance through an anchor and tries to put it into a protocol. Before anything
is signed, the page reads the target contract's bytes from the ledger and asks the verifier
registry what a staked, slashable verifier set concluded about them. `Verified` lets the deposit
through; anything else blocks it and says why.

Deployed at <https://sorofy.site/gate/> — Caddy serves this directory from `/srv/gate` on the host
(see [deploy-playbook.md](../../docs/deploy-playbook.md)). To run it locally instead:

```sh
python3 serve.py          # http://127.0.0.1:8081
node --test test/*.test.js
```

Static files and two pinned CDN modules — no build step, no install, no framework.
`serve.py` is threaded on purpose: `python3 -m http.server` serves one connection at a time and drops modules on a cold load.

## Where it lives, and why

Under `web/`, not `crates/api/static/`. The explorer's assets are `include_str!`'d into the
Rust binary and routed from `server.rs`, so adding a page there means editing
`crates/api/src/`. This is served separately instead.

That also makes a real difference visible rather than hiding it: the explorer has a deliberate
**no-egress** property — bundled fonts, `connect-src 'self'`, nothing cross-origin. This page
cannot have it. It must reach an anchor, a Soroban RPC and a wallet. Its CSP enumerates exactly
those three origins and still forbids inline script, inline style, `eval` and frames.

## What the gate actually does

```
contract id
   │
   ├─ getLedgerEntries  ────────────►  the wasm hash the ledger stores      ← the trust anchor
   │
   ├─ getEvents on the registry ────►  which claims exist for that hash
   │    (filtered on topic 2 = the hash, paged)
   │    └─ or a configured build descriptor, recomputed in-browser
   │
   └─ consensus(wasm_hash, input_digest)  ──►  NoClaim │ Open │ Insufficient
                                               Verified │ Mismatch │ Disputed
```

**Sorofy's API is never consulted.** Not an oversight — it is the point. A lying or compromised
API cannot produce a pass: the hash comes from the chain and the verdict comes from the
registry. The worst a wrong claim can do is name something nobody attested, which reads as
`NoClaim` and blocks.

**Reading stays free, anonymous and token-free** ([§8](../../docs/anchor-integration.md)). The
check needs no wallet, no account, no token and no payment. `consensus()` is *simulated* from a
keypair generated in the page and thrown away — never funded, never used to sign. Verified on
the live network: a read succeeds from an account that does not exist.

**The gate is a control, not a display.** `supply()` re-reads consensus immediately before it
builds the transaction and refuses on its own. A stale panel, an edited DOM or a second tab
cannot carry a pass that was true a minute ago.

## The claim problem, and how it is solved here

The registry files attestations under `(wasm_hash, input_digest)` and offers no
"is anything verified for this hash" query — the gap §4b names. `input_digest` is derived from
the *build* (source, image, flags), which the chain does not carry: neither the VRFY token nor
the Blend v2 pool publishes SEP-58 metadata (checked with `stellar contract info meta`). So
claims are found two ways:

1. **The registry's own `attested` events.** Topic 2 is the wasm hash, so the RPC filters
   server-side. Bounded by event retention — about 121 000 ledgers, a week, measured 2026-09-19.
2. **A configured build descriptor** (`config.js`), recomputed in the browser with WebCrypto,
   for a claim that has aged out of that window.

A descriptor only ever asks a question. The registry always gives the answer.

## What was checked, not assumed

Everything below was verified on **2026-09-19** against the live network, and the inconvenient
answers are recorded as found.

| Question | Answer |
|---|---|
| A testnet anchor doing SEP-24? | **Yes** — `testanchor.stellar.org`: SEP-1 200, `/auth`, `/sep24`, deposit 1–10 units |
| Does it handle TRY? | **No.** SRT, USDC and native only. So: *the SEP-24 flow works end to end against a test anchor; a TRY partner is an integration question.* Never "lira on-ramp" |
| Blend v2 testnet pool | `CCEBVDYM…44HGF`, wasm `a41fc53d…`, matching blend-utils' `lendingPoolV2` |
| Its deposit interface | `submit(from, spender, to, requests)`, `Request { address, amount, request_type }`, `RequestType::Supply = 0` |
| Its reserves | native, wETH, wBTC, USDC — read from `get_reserve_list()` |
| DeFindex testnet | factory `CDSCWE4G…`, USDC vault `CBMVK2JK…`, `deposit(amounts_desired, amounts_min, from, invest)`. Recorded, not wired |
| Does the anchor's USDC compose with Blend's? | **No.** Different issuers — anchor SAC `CBIELTK6…`, Blend's `CAQCFVLO…` |
| What does compose? | **XLM.** The anchor deposits `native`; the pool's first reserve is the native SAC `CDLZFC3S…` |
| Can the browser read Sorofy's API? | **No** — `sorofy.site` sends no CORS headers. The gate does not need it |

## The two paths

| Target | Reads | Why |
|---|---|---|
| Blend v2 pool `CCEBVDYM…` | `NoClaim` → **blocked** | Nobody has attested it. It declares `source_repo` and no SEP-58 build metadata, so today it *cannot* be verified — which is the measurement, not a disappointment |
| VRFY token `CCM2LLD2…` | `Verified` → **through** | Sorofy's own engine rebuilt it and reported `VERIFIED` against the on-chain hash; three staked verifiers attested that claim |

The VRFY token is not a pool, so it demonstrates the verdict rather than a deposit. Point the
gate at a verified *pool* once one exists.

## Configuration

All of it is in [`config.js`](config.js), each value with the check that produced it.

- `ACTIVE_REGISTRY` — defaults to the **rehearsal** instance `CDACBJDL…`, because that is the
  one carrying attestations. The demo instance `CA4VYPAG…` is kept free of test traffic and
  answers `NoClaim` for everything until stream 2 attests something real.
- `ACTIVE_ASSET` — `native`. The only asset that composes (see above). `SRT` is offered because
  it is the case where a **trustline is genuinely required**; native needs none, and the UI says
  so rather than quietly skipping a step.
- `VENDOR` — `@stellar/stellar-sdk@17.1.0` and `@creit.tech/stellar-wallets-kit@2.6.0`, pinned
  exactly. `npm pack` them and serve locally if the venue network is a risk.

## Tests

`node --test test/*.test.js` — 42 tests, no network, no SDK, no dependencies.

The fixtures are captured from the live network rather than invented:
`fixtures.registry-events.json` is 21 events pulled verbatim out of `getEvents`;
`fixtures.stellar.toml` is the anchor's real SEP-1 file.

What they pin:

- The **claim digest** against the same four known-answer vectors the registry
  (`contracts/registry/src/test.rs`) and the API (`crates/api/src/claim.rs`) are pinned to —
  vectors produced independently in Python, so agreement is with a third party, not between two
  readings by one author.
- The **decision table** against consensus values actually read off the chain, including that
  exactly one state opens the gate.
- The **event decoding**, recovering all six walk-through claims from real bytes.
- That `supply` refuses without ever reaching the wallet.
- That an unrecognised value **throws** rather than degrading to "not verified" — a block caused
  by a decoding bug is indistinguishable from a real one, which is the worst failure available.

## What this is not

Stated here because it has to be stated wherever the gate is shown.

- **A verified contract is not a safe contract.** Verification proves the deployed bytes match
  source that staked verifiers rebuilt. It says nothing about whether that source is any good.
  It is the precondition for review, not a substitute: you cannot audit code you cannot prove is
  running.
- **This gate is client-side.** Real check, real consensus, real refusal — and anyone can call
  the pool directly and nothing here stops them. Only §4b would bind.
- **No lira.** See the table above.
- **The gate is optional.** A safer path, not a toll booth. Sorofy takes no fee from any of it.
- **Testnet.** Every asset is worthless and the verifier stake is a valueless token.
- **The three verifiers are three keys on one laptop.** Cross-host independence is stream 2's
  job, and until it is done the consensus is a proof of mechanism, not of decentralisation.
