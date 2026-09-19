/**
 * Every address, endpoint and constant the gate uses.
 *
 * Each one was *checked* on 2026-09-19 rather than copied from memory, and the check is
 * recorded next to it. Where a fact turned out to be inconvenient it is written down anyway —
 * the anchor has no TRY, and the anchor's USDC is not the pool's USDC.
 */

export const NETWORK = {
  passphrase: 'Test SDF Network ; September 2015',
  // The same endpoint crates/api/src/rpc.rs uses.
  rpc: 'https://soroban-testnet.stellar.org',
  horizon: 'https://horizon-testnet.stellar.org',
  explorer: 'https://stellar.expert/explorer/testnet',
};

/**
 * The verifier registry the gate reads its verdict from.
 *
 * Two instances are deployed (contracts/deployments/testnet.json). The rehearsal instance is
 * the default here because it is the one that *has* attestations: the STEP 4 walk-through left
 * six claims on it, covering every consensus state, so the gate can be demonstrated against
 * real on-chain data. The demo instance is deliberately kept free of test traffic, so today it
 * answers `NoClaim` for everything — switch to it once stream 2 has attested something real.
 */
export const REGISTRY = {
  rehearsal: 'CDACBJDL7SEXSODGEAQSK5SPVHWZ7QM5PGQP37C7Y3ACYO5Q5QJQZXY4',
  demo: 'CA4VYPAGEYYOV7CJIBTCJHOGW2KAFQ4AHYEZIY2NA3NXFJGGG4XSPCFE',
};
export const ACTIVE_REGISTRY = REGISTRY.rehearsal;

/**
 * How far back to scan the registry's `attested` events when discovering which claims exist
 * for a wasm hash.
 *
 * The RPC's own `oldestLedger` was 4 640 730 against a latest of 4 761 689 on 2026-09-19 —
 * about 121 000 ledgers, roughly a week. It also pages: a request covering 16 000 ledgers
 * returned an empty first page *with a cursor*, while one covering 8 000 returned the events
 * directly. So the scan follows the cursor rather than trusting one page (see chain.js).
 */
export const EVENT_SCAN = {
  maxLedgersBack: 100_000,
  pageLimit: 200,
  maxPages: 40,
};

/**
 * The anchor.
 *
 * Checked live: `https://testanchor.stellar.org/.well-known/stellar.toml` returns 200 and
 * advertises SEP-10 (`/auth`), SEP-12, SEP-24 (`/sep24`), SEP-31 and SEP-38.
 *
 * **It does not handle TRY.** Its `[[CURRENCIES]]` are SRT, USDC and native, and
 * `GET /sep24/info` enables deposit for exactly those three, 1–10 units each. So the honest
 * claim is the one docs/anchor-integration.md §9 prescribes: *the deposit flow works end to
 * end against a test anchor; a TRY partner is an integration question.* Not "lira on-ramp".
 */
export const ANCHOR = {
  homeDomain: 'testanchor.stellar.org',
  // SEP-1 is fetched at run time; these are only what to expect from it.
  expected: {
    webAuth: 'https://testanchor.stellar.org/auth',
    sep24: 'https://testanchor.stellar.org/sep24',
    signingKey: 'GCHLHDBOKG2JWMJQBTLSL5XG6NO7ESXI2TAQKZXCXWXB5WI2X6W233PR',
  },
};

/**
 * The asset the flow uses.
 *
 * `native` is not a default for want of thinking about it — it is the only asset that actually
 * composes. The anchor's USDC is issued by GBBD47IF…, whose SAC is
 * `CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA`; the Blend pool's USDC reserve is
 * `CAQCFVLOBK5GIULPNZRGATJJMIZL5BSP7X5YJVMGCPTUEPFM4AVSRCJU`, a different issuer entirely.
 * They are not the same token and one cannot be supplied in place of the other.
 *
 * XLM is the overlap: the anchor deposits `native`, and the pool's first reserve is the native
 * SAC `CDLZFC3S…` (confirmed with `stellar contract id asset --asset native`). So the chain
 * anchor → balance → pool holds for XLM and for nothing else here.
 *
 * SRT is offered as an alternative because it is the case where a **trustline is actually
 * needed** — native needs none — so the SEP-24 half can be shown in full. It cannot then be
 * supplied to the pool, and the UI says so rather than pretending.
 */
export const ASSETS = {
  native: {
    code: 'native',
    label: 'XLM',
    issuer: null, // native needs no trustline
    sac: 'CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC',
    decimals: 7,
    depositable: true,
    suppliable: true,
  },
  SRT: {
    code: 'SRT',
    label: 'SRT',
    issuer: 'GCDNJUBQSX7AJWLJACMJ7I4BC3Z47BQUTMHEICZLE6MU4KQBRYG5JY6B',
    sac: 'CBZVLMD5DBKIFSVU23WAVMJD25IRUBWCVAXGURPUPMB2CUNFBQN742UV',
    decimals: 7,
    depositable: true,
    // The pool has no SRT reserve, so a deposit of SRT has nowhere to go.
    suppliable: false,
  },
};
export const ACTIVE_ASSET = 'native';

/**
 * The protocols a deposit can be gated into.
 *
 * Blend v2 is the one implemented. Its pool, reserve list and entry point were read from the
 * chain, not from documentation:
 *
 *   - `CCEBVDYM…` runs wasm `a41fc53d6753b6c04eb15b021c55052366a4c8e0e21bc72700f461264ec1350e`,
 *     which matches `lendingPoolV2` in blend-capital/blend-utils' testnet.contracts.json.
 *   - `get_reserve_list()` returns [native, wETH, wBTC, USDC].
 *   - The supply entry point is `submit(from, spender, to, requests)` where a request is
 *     `{ address, amount, request_type }`, and `RequestType::Supply = 0` in
 *     blend-contracts-v2/pool/src/pool/actions.rs.
 *
 * Its contract metadata carries `source_repo: github:blend-capital/blend-contracts-v2` and no
 * SEP-58 build fields — which is exactly why the gate blocks it. That is not a flaw in the
 * demo, it is the measurement.
 *
 * DeFindex is recorded, not implemented: the vault's entry point is
 * `deposit(amounts_desired, amounts_min, from, invest)` (read from the chain), and the testnet
 * ids come from paltalabs/defindex's public/testnet.contracts.json.
 */
export const PROTOCOLS = {
  blendV2Pool: {
    kind: 'blend-v2',
    label: 'Blend v2 — testnet pool',
    contractId: 'CCEBVDYM32YNYCVNRXQKDFFPISJJCV557CDZEIRBEE4NCV4KHPQ44HGF',
    note: 'A live Blend v2 lending pool. Declares a source repo, no SEP-58 build metadata.',
  },
  defindexUsdcVault: {
    kind: 'defindex',
    label: 'DeFindex — USDC vault (recorded, not wired)',
    contractId: 'CBMVK2JK6NTOT2O4HNQAIQFJY232BHKGLIMXDVQVHIIZKDACXDFZDWHN',
    note: 'deposit(amounts_desired, amounts_min, from, invest). Left unimplemented on purpose.',
    unimplemented: true,
  },
  vrfyToken: {
    kind: 'none',
    label: 'VRFY token — Sorofy’s own, reproduced by its own engine',
    contractId: 'CCM2LLD2TAFOMQQUQK62DHCEWNO7UEYOWDXL52GF3KTDK4NMBT6HKEDW',
    note: 'Not a pool — here so the gate can be pointed at a contract Sorofy really rebuilt.',
    noDeposit: true,
  },
};
export const ACTIVE_PROTOCOL = 'blendV2Pool';

/**
 * Build descriptors, for contracts whose SEP-58 inputs are on record.
 *
 * The registry files attestations under `(wasm_hash, input_digest)`, and `input_digest` is
 * derived from the *build* — source, image, flags — which the chain does not carry: neither the
 * VRFY token nor the Blend pool publishes SEP-58 metadata. So when an attestation is too old to
 * still be in the RPC's event window, the only way to name the claim is to know the build.
 *
 * These come from contracts/deployments/testnet.json. The digest is recomputed in the browser
 * from them (lib/claim.js) and checked against the registry — the descriptor names a claim, it
 * never asserts a verdict.
 */
export const BUILD_DESCRIPTORS = {
  // wasm hash of the deployed VRFY token → how it was built.
  '3e23ccf50f1a28a568229e21eddc2f11ea6ee495ebe2c3736e545d054be59028': {
    sourceSha256: '47a88a77289c02370d0f04995e6faadcea4f5fb081981ff5ee48762804c0f306',
    bldimgDigest: 'cff44167d2ee90f901c768ccdb75eb5c382c95295f44bcd7ee4b543f0adf9588',
    bldopt: ['--package=vrfy-token'],
    note: 'VRFY token, reproduced by Sorofy’s engine and VERIFIED against the on-chain hash.',
  },
};

/**
 * Pinned library versions.
 *
 * Exact, never a floating range: a breaking release on the morning of a demo is a failure mode
 * with no upside. Both were confirmed to resolve on jsDelivr on 2026-09-19.
 *
 * Note what this costs. The Sorofy explorer bundles its fonts and talks to nothing but its own
 * origin — a deliberate no-egress property. This page cannot have that property: it has to
 * reach an anchor, an RPC and a wallet. It is a different kind of page and it should not be
 * described as if it were the same one. `npm pack` those two versions and serve them locally if
 * the venue network is a risk.
 */
export const VENDOR = {
  stellarSdk: 'https://cdn.jsdelivr.net/npm/@stellar/stellar-sdk@17.1.0/+esm',
  walletsKit: 'https://cdn.jsdelivr.net/npm/@creit.tech/stellar-wallets-kit@2.6.0/+esm',
};
