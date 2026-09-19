/**
 * What the registry's answer means, and what the gate does about it.
 *
 * Kept free of the SDK and of the DOM on purpose: this is the part that decides whether money
 * moves, so it is the part that has to be testable without a browser or a network. `chain.js`
 * fetches the value; this file is the only place that judges it.
 *
 * The six states are the registry's own (`Consensus` in contracts/registry/src/lib.rs). Only
 * one of them opens the gate, and the reasoning for that is in docs/hackathon-design.md §6:
 * consensus is *conservative*, so a green result needs every attester to agree. Anything else —
 * including "nobody has looked yet" — is not a green result and must not be shown as one.
 */

/** The only state that opens the gate. */
export const VERIFIED = 'Verified';

/** Every state the registry can return, in the order it defines them. */
export const STATES = ['NoClaim', 'Open', 'Insufficient', VERIFIED, 'Mismatch', 'Disputed'];

/**
 * Normalize whatever the SDK hands back into `{ state, tally }`.
 *
 * A Soroban enum with payloads decodes to `[name, payload]` and a unit variant to `[name]`, but
 * the shape depends on the SDK version and on whether something has already been through JSON.
 * Accepting the three shapes that actually occur costs a few lines and removes a class of
 * silent failure — and an unrecognised shape throws rather than degrading to "not verified",
 * because a bug must not be able to masquerade as a legitimate block.
 */
export function normalizeConsensus(raw) {
  // ["Disputed", {distinct, top_count, total}] / ["NoClaim"]
  if (Array.isArray(raw) && typeof raw[0] === 'string') {
    return { state: assertState(raw[0]), tally: normalizeTally(raw[1]) };
  }
  // "Verified"
  if (typeof raw === 'string') {
    return { state: assertState(raw), tally: null };
  }
  // {"Disputed": {distinct, top_count, total}}
  if (raw && typeof raw === 'object') {
    const keys = Object.keys(raw);
    if (keys.length === 1) {
      return { state: assertState(keys[0]), tally: normalizeTally(raw[keys[0]]) };
    }
  }
  throw new Error(`unrecognised consensus value: ${JSON.stringify(raw)?.slice(0, 120)}`);
}

function assertState(name) {
  if (!STATES.includes(name)) {
    throw new Error(`unknown consensus state: ${String(name).slice(0, 40)}`);
  }
  return name;
}

function normalizeTally(t) {
  if (!t || typeof t !== 'object') return null;
  const num = (v) => (typeof v === 'bigint' ? Number(v) : typeof v === 'number' ? v : null);
  const distinct = num(t.distinct);
  const top = num(t.top_count ?? t.topCount);
  const total = num(t.total);
  if (distinct === null && top === null && total === null) return null;
  return { distinct, top, total };
}

/**
 * The gate's decision.
 *
 * `allow` is true for exactly one state. Everything else carries a reason the user can act on,
 * because "blocked" with no explanation is indistinguishable from a broken page — and the
 * point of the gate is not to refuse, it is to tell someone what is and is not known about the
 * code their money is about to enter.
 */
export function decide({ state, tally }) {
  const n = tally ?? { distinct: null, top: null, total: null };
  const counted = n.total === null ? 'no' : String(n.total);

  switch (state) {
    case VERIFIED:
      return {
        allow: true,
        tone: 'ok',
        headline: 'Verified',
        detail:
          'Every verifier that attested this build rebuilt the exact bytes deployed on chain, ' +
          'the attestation window has closed, and they are staked against being wrong.',
      };

    case 'NoClaim':
      return {
        allow: false,
        tone: 'blocked',
        headline: 'Not verified',
        detail:
          'No staked verifier has attested this contract. That is not an accusation — it is ' +
          'the absence of evidence. Nothing is known about whether the deployed bytes match ' +
          'any published source.',
      };

    case 'Open':
      return {
        allow: false,
        tone: 'waiting',
        headline: 'Still being attested',
        detail:
          `The attestation window is still open with ${counted} attestation(s) so far. ` +
          'A result is only green once the window has closed, so that a late dissent cannot ' +
          'arrive after someone has already acted on it.',
      };

    case 'Insufficient':
      return {
        allow: false,
        tone: 'waiting',
        headline: 'Not enough verifiers',
        detail:
          `Only ${counted} verifier(s) attested before the window closed, short of the quorum ` +
          'this registry requires. One verifier agreeing with itself is not a consensus.',
      };

    case 'Mismatch':
      return {
        allow: false,
        tone: 'danger',
        headline: 'Does not match its source',
        detail:
          'Verifiers rebuilt the published source and got different bytes from the ones ' +
          'deployed. This is the failure the whole system exists to catch.',
      };

    case 'Disputed':
      return {
        allow: false,
        tone: 'danger',
        headline: 'Verifiers disagree',
        detail:
          `${counted} verifier(s) attested and they did not all rebuild the same hash ` +
          `(${n.distinct ?? '?'} distinct results, the most common held by ${n.top ?? '?'}). ` +
          'Any dissent at all blocks a green result, and whoever is outvoted can be slashed.',
      };

    default:
      throw new Error(`no decision defined for state ${state}`);
  }
}

/**
 * The single most favourable decision across every claim filed for one wasm hash.
 *
 * The registry files attestations under `(wasm_hash, input_digest)`, so one deployed hash can
 * carry several claims — the same bytes rebuilt from different declared inputs. There is no
 * `any_verified(wasm_hash)` entry point, so the gate asks about each claim it can find and
 * takes the best answer, which is the "some verified claim for this hash" notion
 * docs/anchor-integration.md §4b says is needed.
 *
 * With no claims at all the answer is `NoClaim`, which is the honest reading of an empty
 * registry: nobody has said anything about this contract.
 */
export function bestOf(results) {
  if (!results || results.length === 0) {
    return { ...decide({ state: 'NoClaim', tally: null }), state: 'NoClaim', claim: null };
  }
  // Rank by how much the state tells a depositor, not alphabetically. A Mismatch or Disputed
  // outranks NoClaim when both are present: "we looked and it is wrong" is worth surfacing
  // over "some other claim has no attestations".
  const rank = { NoClaim: 0, Open: 1, Insufficient: 2, Disputed: 3, Mismatch: 4, Verified: 5 };
  const best = results.reduce((a, b) => (rank[b.state] > rank[a.state] ? b : a));
  return { ...decide(best), state: best.state, tally: best.tally ?? null, claim: best.claim ?? null };
}
