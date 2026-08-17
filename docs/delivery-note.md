# Sorofy — Delivery Note

**Engagement:** Instawards SOW, 30 days · **Live service:** <https://sorofy.site> ·
**Date:** 2026-08-16

---

## The problem, in one paragraph

When a smart contract is deployed to Stellar, what actually lands on the network is a
block of compiled bytes. Explorers show source code next to those bytes, but nothing
proves the two match — the code you read may not be the code that runs. Sorofy closes
that gap: it takes the published source, rebuilds it in a locked-down environment, and
checks whether the result is byte-for-byte identical to what is on-chain.

## What was funded, and where it stands

| # | Deliverable | Status |
|---|---|---|
| 1 | A build engine that rebuilds a contract from source and compares it to the on-chain bytes | ✅ Built and running |
| 2 | A public API, live on testnet, that anyone can query | ✅ Live at <https://sorofy.site> — two real testnet contracts checked through it |
| 3 | A path for contracts that carry no source information on-chain | ✅ Proven |

All three are delivered. Everything below can be checked without installing anything.

## How to verify it yourself, in 30 seconds

**Open this link:** [a verified contract](https://sorofy.site/#/v/CAZAVVTM3GXFNCLR66FYHJJ43MEEUV3C6PQYRQT5JVGAO2RS6S4OHRT6)
· and [a second one](https://sorofy.site/#/v/CAEA4BXANQ2JQR4AF5XG53A25LU5N2QERRFC5P7ZY4W6YDQ4DGLEZRYH)

You will see two long strings of letters and numbers:

- **On chain (expected)** — the fingerprint of the contract **as it exists on the
  Stellar network**. Sorofy reads this from the network itself; the person requesting
  the check cannot supply it or influence it.
- **Rebuilt here** — the fingerprint of what Sorofy got when it **rebuilt the
  contract from its published source code**.

They are identical, and the status reads `verified`. That is the whole product: the
source really does produce those bytes.

You can also browse <https://sorofy.site> in a normal browser for a readable view of
every check the service has run. The same addresses answer with raw JSON when a program
asks for it instead of a browser — that is how an explorer or wallet would query them.

## That the check has teeth

A verifier that always says "yes" is worthless. So the same engine was pointed at a
copy of a contract with **one word changed** — the text `"Hello"` replaced with
`"Howdy"` — and asked to match the original.

It refused. The altered version compiles to a file of **exactly the same size**, 660
bytes, so nothing about its shape gives it away — but the fingerprint is completely
different, and the engine reported `MISMATCH`. A single character anywhere in the
source is enough to break the match. That is what makes a `verified` meaningful.

## What the service does about security

It compiles code submitted by strangers, which is inherently dangerous, so the
safeguards were designed first and reviewed independently:

- The compile step runs **with no network access at all**, as a non-privileged user,
  with hard limits on memory, processors and process count.
- The one step that does need the internet (downloading dependencies) is **firewalled
  away from anything internal** — including the cloud metadata service, the single
  most valuable target on any server. This was tested live: internal addresses fail,
  legitimate ones succeed.
- Requests that start work require a **secret token**; reading results stays public.
  Flooding the service is throttled automatically.
- All traffic is **encrypted** (HTTPS), and the service itself is unreachable except
  through that encrypted front door.

A written threat model and a register of every known gap — including the ones still
open — is kept in the repository. Nothing was quietly dropped.

## Numbers from the live run

| | |
|---|---|
| Checks run through the live service | 24 |
| Correctly reported `verified` | 19 |
| Correctly reported `mismatch` (deliberate tests) | 4 |
| Correctly **refused** (deliberate test: a build environment that could be swapped after the fact) | 1 |
| Wrong answers | 0 |
| Average time to rebuild and check a contract | 85 seconds |

Anyone can recompute this list at <https://sorofy.site/verifications> — it is the
service's own record, not a figure typed into a document.

## What comes next, and is not part of this engagement

Three things are designed and documented but deliberately out of scope here: trust
tiers for build environments, a registry for older contracts that cannot carry source
information, and multiple independent verifiers that can disagree with each other.
The last is the hardest and the most valuable — it removes the need to trust Sorofy
itself — and it is the headline item of the next milestone.

---

*Full technical detail: [README](../README.md) · roadmap and live status:
[testnet-roadmap.md](testnet-roadmap.md) · security model:
[security.md](security.md) · deploy procedure: [deploy-playbook.md](deploy-playbook.md)*
