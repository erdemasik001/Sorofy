# Post-hackathon roadmap — toward the SCF Build Award

> Scale-track requirement. Written **2026-09-20**. Every "done" below points at something that
> was run, deployed or is live; everything else is dated as an estimate and says so.
>
> Two funding lines are easy to confuse, so they are named apart throughout:
> **Instawards** is the 30-day SOW that funded the *existing* service
> ([delivery note](delivery-note.md), submitted 2026-07-18). **SCF** is the Stellar Community
> Fund Build Award that is the target ahead.

## The short version

The Instawards SOW declared five things out of scope by name: **multi-verifier /
decentralisation, an on-chain registry contract, mainnet + audit, explorer/wallet UI, and
guaranteed determinism across all contracts**
([testnet-roadmap.md](testnet-roadmap.md)).

**This hackathon built the first two of those five.** That is the whole claim of the delta: it
closes *most* of the gap between what the funded SOW delivered and what the SCF M2 milestone
asks for — a registry contract deployed to testnet with a live verdict anyone can read without
an account, not a prototype in a branch. One piece stays open, and it is named below: **follower
mode**.

## Where each milestone actually stands

SCF milestone names from the RFP track. Build Award cap (**up to $150,000 worth of XLM**) and
tranche structure (**10 / 20 / 30 / 40** at award acceptance / MVP / testnet / mainnet launch)
from the SCF handbook,
[Budget and deliverable guidelines](https://stellar.gitbook.io/scf-handbook/scf-awards/build-award/budget-and-deliverable-guidelines).

| SCF milestone | What it asks for | Status | What delivered it |
|---|---|---|---|
| **M0 — Acceptance** | Interest form, architecture, "why us" | 🔄 **ready to submit** | [Architecture](hackathon-architecture.md) drawn from running code; this roadmap; the delta in [HACKATHON.md](../HACKATHON.md) |
| **M1 — MVP** | Testnet verifier, public API, Docker reproduction | ✅ **done, and live** | **Instawards SOW.** Deterministic engine in a digest-pinned image; public REST API live at [sorofy.site](https://sorofy.site) with `trust_level` in the schema; two distinct testnet contracts verified against RPC-resolved hashes |
| **M2 — Testnet** | Decentralisation (multi-verifier), retroactive verification, registry | ⚠️ **mostly done — one named gap** | Retroactive path: **Instawards SOW**. Registry + multi-verifier staking, conservative consensus and slash: **this hackathon** — deployed, 46 contract tests, `Verified` readable on-chain today. **Missing: follower mode** |
| **M3 — Mainnet** | Audit prep → Audit Bank review → production, plus ≥1 reference integration | ⛔ **not started** | The Blend v2 gate is a working reference integration *on testnet*, which is the shape M3 wants — but no mainnet deploy, no audit, and VRFY is a worthless token |

## What has to close before M2 can be called complete

In the order that actually unblocks the next one.

| # | Gap | Why it matters | Estimate |
|---|---|---|---|
| 1 | **Follower mode** | Without it, "several verifiers" is a manual act, not a mechanism. An independent operator has no way to *get work*. It is the single thing standing between the registry and a genuinely decentralised claim. Specified in [design §8](hackathon-design.md); zero implementation | 1–2 weeks |
| 2 | **A real stake asset** | VRFY is valueless, so today's slash deters nothing. Economic security needs an asset with a price, and that is a design decision with governance attached — not a code change | Design first, then 2–3 weeks |
| 3 | **Explorer rows for stakes and attestations** | The verdict is readable from the chain but not from Sorofy's own explorer. Named in the hackathon plan, not built | 3–5 days |
| 4 | **On-chain gate wrapper** ([§4b](anchor-integration.md)) | The gate is client-side: it refuses in earnest, but anyone can call the pool directly. A contract-level wrapper is what makes the refusal binding | 2–3 weeks, separate spike |
| 5 | **Independent operators** | Nobody outside this project has run a verifier. Until one does, "multi-verifier" is three accounts on one machine — and that limitation is recorded rather than hidden | Depends on (1) |

Gap 1 is the one that changes the story; the rest are increments.

## Toward M3

- **Audit.** Mandatory third-party audit, cost covered separately by the [Soroban Audit Bank](https://stellar.gitbook.io/scf-handbook/supporting-programs/audit-bank). The registry contract is the surface that needs it — 18,746 bytes of wasm, 35 unit tests, 7 of 7 deliberate mutations caught. Audit readiness is mostly a documentation exercise from here.
- **Reference integration.** The RFP wants at least one. Blend v2 is already wired on testnet and the pool call is load-bearing. Moving it to mainnet is the work; finding a partner is not.
- **SEP-58 dependency.** The one external unknown that is not ours to close. The Blend v2 pool publishes `source_repo` and **no SEP-58 build metadata** — checked with `stellar contract info meta`. Until contracts routinely publish it, a verifier has to be handed build descriptors out of band, which is what `config.js` does today and does not scale. SEP-58 is still Draft.

## Risks, updated by what the hackathon actually measured

The pre-hackathon risk register ([idea1-project-brief.md](../idea1-project-brief.md) §9) held up.
Two entries changed:

| Risk | Then | Now |
|---|---|---|
| Rust/host non-determinism | Mitigated by container | **Confirmed real and sharper than expected.** The same source built with local stellar-cli 28.0.0 and with the pinned image's 23.2.1 produces *different bytes*. Only the pinned build reproduces. The image digest is not a nicety, it is the whole guarantee |
| SEP-58 still Draft | "Track the spec closely" | **Now measurable.** A live, widely-used pool carries no SEP-58 metadata at all, so the gap is not theoretical. Early-mover advantage stands |
| Competition (SoroSeal, stellar-cli, SDF prototype) | Biggest business risk | Unchanged. Nothing in this hackathon addresses it; differentiation is still multi-verifier + slashing, which is now built rather than argued |

## What is not known, and where to find out

Recorded rather than guessed.

- **Which SCF round is open.** The pre-hackathon brief flags rounds #43 and #44 as conflicting and says to clarify via the interest form. Not resolved here — an attempt to read `communityfund.stellar.org/awards` during this session was blocked before the page loaded. **How to settle it:** the SCF interest form at [communityfund.stellar.org](https://communityfund.stellar.org/awards), which is also where a referral code would be attached. **Indicative, not confirmed:** the handbook and public sources put rounds at roughly six-week intervals with SCF #41 closing 2026-02-01, which would place #43 and #44 months in the past by today — so the brief's "#43 vs #44" is more likely *stale* than genuinely ambiguous. That was not read from the source of truth: `communityfund.stellar.org` does not resolve from this machine (`getaddrinfo ENOTFOUND`, re-checked 2026-09-20).
- **Whether the Blend v2 team would accept a gated deposit path.** No contact has been made. The integration is ours, built against their public interface; it is not a partnership and is not presented as one.

## What this roadmap deliberately does not promise

- No verifier rewards. The reward model is a projection ([verifier-economics.md](verifier-economics.md)); no funding is assumed and none has been awarded.
- No mainnet date. M3 depends on an audit that has not been scheduled.
- No fiat rail. The SEP-24 on-ramp works against a test anchor; **no Turkish lira partner is integrated**, and a TRY path remains an integration question rather than a capability.
