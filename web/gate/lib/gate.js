/**
 * The gate itself: from a contract id to a decision about it.
 *
 * Sequence, and why it is in this order:
 *
 *   1. Ask the **chain** which bytes the contract runs. Not the caller, not Sorofy's API —
 *      this is the one value everything else is about, so it comes from the ledger.
 *   2. Find which claims have been filed for those bytes. The registry files attestations under
 *      `(wasm_hash, input_digest)` and offers no "anything for this hash?" query, so the claims
 *      are discovered from its own `attested` events, and a configured build descriptor can
 *      name one that has aged out of the event window.
 *   3. Read `consensus()` for each claim, and take the best answer.
 *
 * The API is never consulted. That is not an oversight: it means a lying Sorofy cannot produce
 * a pass. The worst a wrong claim can do is name something nobody attested, which reads as
 * `NoClaim` and blocks.
 *
 * Every step works with no wallet, no account and no token.
 */

import { ACTIVE_REGISTRY, BUILD_DESCRIPTORS } from '../config.js';
import { inputDigestHex } from './claim.js';
import { discoverClaims, executableOf, readConsensus } from './chain.js';
import { bestOf, decide } from './consensus.js';

/**
 * Check one contract.
 *
 * Returns `{ ok, allow, headline, detail, wasmHash, claims, checked, source }`. `ok: false`
 * means the check could not be completed — a contract that does not exist, an RPC that would
 * not answer — which is reported as its own thing rather than as a block, because "we could not
 * look" and "we looked and it is not verified" are different sentences.
 */
export async function checkContract(contractId, { registryId = ACTIVE_REGISTRY } = {}) {
  let executable;
  try {
    executable = await executableOf(contractId);
  } catch (e) {
    return unavailable(`could not read ${short(contractId)} from the network: ${e.message}`);
  }

  if (!executable) {
    return unavailable(`no contract ${short(contractId)} exists on this network`);
  }
  if (executable.stellarAsset) {
    return unavailable(
      'this is a built-in Stellar Asset Contract — it runs no WASM, so there is nothing to ' +
        'rebuild and nothing to verify',
    );
  }

  const { wasmHash } = executable;
  const candidates = await claimsFor(registryId, wasmHash);

  const results = [];
  for (const claim of candidates) {
    try {
      const consensus = await readConsensus(registryId, wasmHash, claim.inputDigest);
      results.push({ ...consensus, claim });
    } catch (e) {
      // One unreadable claim must not decide the answer for the others.
      results.push({ state: 'NoClaim', tally: null, claim: { ...claim, error: e.message } });
    }
  }

  const verdict = bestOf(results);
  return {
    ok: true,
    ...verdict,
    contractId,
    wasmHash,
    registryId,
    claims: results,
    checked: new Date().toISOString(),
  };
}

/**
 * Which claims to ask about, from both sources, newest evidence first.
 *
 * A descriptor is only ever a *question* — "is this particular build attested?" — never an
 * answer. The registry still decides.
 */
async function claimsFor(registryId, wasmHash) {
  const byDigest = new Map();

  try {
    for (const claim of await discoverClaims(registryId, wasmHash)) {
      byDigest.set(claim.inputDigest, { ...claim, source: 'chain events' });
    }
  } catch {
    // The scan is a convenience, not a requirement; a descriptor may still name the claim.
  }

  const descriptor = BUILD_DESCRIPTORS[wasmHash.toLowerCase()];
  if (descriptor) {
    try {
      const inputDigest = await inputDigestHex(descriptor);
      if (!byDigest.has(inputDigest)) {
        byDigest.set(inputDigest, { wasmHash, inputDigest, source: 'configured build' });
      }
    } catch {
      // A descriptor that does not produce a digest is a configuration error, not a verdict.
    }
  }
  return [...byDigest.values()];
}

function unavailable(reason) {
  return {
    ok: false,
    allow: false,
    tone: 'unknown',
    headline: 'Cannot be checked',
    detail: reason,
    claims: [],
    wasmHash: null,
  };
}

function short(id) {
  return typeof id === 'string' && id.length > 12 ? `${id.slice(0, 6)}…${id.slice(-4)}` : String(id);
}

/**
 * The sentence that has to appear wherever a green result does.
 *
 * docs/anchor-integration.md §9 calls this the point most likely to be tested by a judge, and
 * it is the one the product cannot deliver if it is left off: verification proves the deployed
 * bytes match public source, and says nothing at all about whether that source is any good.
 */
export const VERIFIED_IS_NOT_SAFE =
  'Verified means the deployed bytes match source that staked verifiers rebuilt. It is not an ' +
  'audit: it says nothing about whether that source is any good. You cannot audit code you ' +
  'cannot prove is running — this is the precondition, not the review.';

/** A decision for a target that has no deposit path, used by the explorer-only entries. */
export const notADeposit = () => decide({ state: 'NoClaim', tally: null });
