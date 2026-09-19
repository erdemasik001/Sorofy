/**
 * The discovery path, against bytes captured from the live network.
 *
 * `fixtures.registry-events.json` is 21 events pulled verbatim out of
 * `getEvents` for the rehearsal registry `CDACBJDL…` on 2026-09-19 — the STEP 4 walk-through
 * plus the STEP 5 live attestation. Decoding those bytes correctly is what lets the gate find
 * which claims exist for a hash, so it is pinned here rather than trusted.
 *
 * Nothing in this file touches the network or the SDK: `chain.js` keeps the SDK import lazy
 * precisely so this is possible.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

import {
  bytesScValBase64,
  claimFromEvent,
  claimsForHash,
  cursorLedger,
  executableFromEntryJson,
} from '../lib/chain.js';
import { decodeScVal } from '../lib/xdr.js';

const FIXTURE = JSON.parse(
  readFileSync(new URL('./fixtures.registry-events.json', import.meta.url), 'utf8'),
);
const EVENTS = FIXTURE.events;

/** The claim the evidence records as Verified: three verifiers, all agreeing. */
const VERIFIED_CLAIM = { wasm: 'a2'.repeat(32), digest: 'b2'.repeat(32) };

test('the captured fixture is the walk-through it claims to be', () => {
  assert.equal(EVENTS.length, 21);
  const names = EVENTS.map((e) => decodeScVal(e.topic[0]));
  assert.equal(names.filter((n) => n === 'attested').length, 13);
  assert.ok(names.includes('staked') && names.includes('slashed'));
});

test('an attested event decodes into the claim it recorded', () => {
  const attested = EVENTS.filter((e) => decodeScVal(e.topic[0]) === 'attested');
  const claim = claimFromEvent(attested[0]);

  assert.match(claim.wasmHash, /^[0-9a-f]{64}$/);
  assert.match(claim.inputDigest, /^[0-9a-f]{64}$/);
  assert.match(claim.rebuiltHash, /^[0-9a-f]{64}$/);
  assert.match(claim.verifier, /^[0-9a-f]{64}$/);
  assert.equal(typeof claim.ledger, 'number');
});

test('events that are not attestations are not claims', () => {
  for (const event of EVENTS) {
    const name = decodeScVal(event.topic[0]);
    if (name === 'attested') continue;
    assert.equal(claimFromEvent(event), null, `${name} should not decode as a claim`);
  }
});

test('the six walk-through claims are recovered, and only those', () => {
  const byHash = new Map();
  for (const event of EVENTS) {
    const claim = claimFromEvent(event);
    if (!claim) continue;
    byHash.set(claim.wasmHash, (byHash.get(claim.wasmHash) ?? 0) + 1);
  }
  // Five synthetic claims from STEP 4 plus the one STEP 5 wrote through the API's own path.
  assert.equal(byHash.size, 6);
  // Claim 2 is the Verified one and it took three attestations to get there.
  assert.equal(byHash.get(VERIFIED_CLAIM.wasm), 3);
});

test('filtering by wasm hash returns that hash’s claims and no others', () => {
  const claims = claimsForHash(EVENTS, VERIFIED_CLAIM.wasm);
  assert.equal(claims.length, 1, 'three attestations, but one claim');
  assert.equal(claims[0].inputDigest, VERIFIED_CLAIM.digest);

  // Case is not significant on the way in; the claim is still found.
  assert.equal(claimsForHash(EVENTS, VERIFIED_CLAIM.wasm.toUpperCase()).length, 1);
  // A hash nobody attested yields nothing, which is the honest answer, not an error.
  assert.deepEqual(claimsForHash(EVENTS, 'ff'.repeat(32)), []);
  assert.deepEqual(claimsForHash([], VERIFIED_CLAIM.wasm), []);
});

test('one unreadable event does not sink the scan', () => {
  // A future contract version could publish a shape this reader does not know. Losing that one
  // event is acceptable; losing the whole scan would turn a parsing gap into a verdict.
  const poisoned = [{ ledger: 1, topic: ['bm90IHZhbGlk', 'bm90', 'bm90'], value: 'bm90' }, ...EVENTS];
  assert.equal(claimsForHash(poisoned, VERIFIED_CLAIM.wasm).length, 1);
});

test('the topic filter encodes the hash exactly as the chain stores it', () => {
  // Checked against the live RPC on 2026-09-19: this filter returned the 3 attestations of
  // claim 2 and nothing else.
  assert.equal(
    bytesScValBase64(VERIFIED_CLAIM.wasm),
    'AAAADQAAACCioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKiog==',
  );
  // And it is the same encoding the captured topic actually carries.
  const attested = EVENTS.find((e) => decodeScVal(e.topic[0]) === 'attested');
  assert.equal(bytesScValBase64(decodeScVal(attested.topic[2])), attested.topic[2]);
});

test('a cursor resolves to the ledger it points past', () => {
  // TOID = ledger << 32 | ..., so the ledger is the top half.
  const toid = (BigInt(4761510) << 32n).toString();
  assert.equal(cursorLedger(`${toid}-1`), 4761510);
  for (const bad of [null, undefined, '', 'abc-1', 42]) {
    assert.equal(cursorLedger(bad), null, `${bad} is not a cursor`);
  }
});

test('the ScVal reader refuses what it does not understand', () => {
  // Type 9 (i128) is real XDR this reader does not implement. It must throw, not return a
  // plausible-looking value that would end up naming a claim.
  const i128 = btoa(String.fromCharCode(0, 0, 0, 9, 0, 0, 0, 0));
  assert.throws(() => decodeScVal(i128), /unsupported ScVal type 9/);
  assert.throws(() => decodeScVal(btoa(String.fromCharCode(0, 0, 0, 13, 0, 0, 0, 32))), /ran off the end/);
});

/* ── the trust anchor: which bytes a contract runs ──────────────────────────────────── */

/**
 * The two executable shapes, captured on 2026-09-19 by decoding real `getLedgerEntries`
 * responses in the browser. Reading these correctly is what makes the gate's subject the
 * contract's actual bytes rather than something a caller asserted.
 */
const ENTRY = {
  blendPool: { contract_data: { val: { contract_instance: { executable: {
    wasm: 'a41fc53d6753b6c04eb15b021c55052366a4c8e0e21bc72700f461264ec1350e' } } } } },
  vrfyToken: { contract_data: { val: { contract_instance: { executable: {
    wasm: '3e23ccf50f1a28a568229e21eddc2f11ea6ee495ebe2c3736e545d054be59028' } } } } },
  nativeSac: { contract_data: { val: { contract_instance: { executable: 'stellar_asset' } } } },
};

test('a deployed contract reports the wasm hash the network stores', () => {
  // Both hashes are independently on record: the Blend one matches blend-utils' lendingPoolV2,
  // the VRFY one matches contracts/deployments/testnet.json.
  assert.deepEqual(executableFromEntryJson(ENTRY.blendPool), {
    wasmHash: 'a41fc53d6753b6c04eb15b021c55052366a4c8e0e21bc72700f461264ec1350e',
  });
  assert.deepEqual(executableFromEntryJson(ENTRY.vrfyToken), {
    wasmHash: '3e23ccf50f1a28a568229e21eddc2f11ea6ee495ebe2c3736e545d054be59028',
  });
});

test('a built-in asset contract is reported as such, not as a hash', () => {
  // A SAC runs no WASM, so there is nothing to rebuild and nothing to verify — a different
  // answer from "not verified", and the gate says so.
  assert.deepEqual(executableFromEntryJson(ENTRY.nativeSac), { stellarAsset: true });
  // The object form some SDK builds produce means the same thing.
  assert.deepEqual(
    executableFromEntryJson({ contract_data: { val: { contract_instance: {
      executable: { stellar_asset: {} } } } } }),
    { stellarAsset: true },
  );
});

test('an executable this reader does not understand throws rather than yielding a hash', () => {
  // Returning a wrong or empty hash here would send the registry a question about bytes that
  // are not the contract's, and the resulting NoClaim would read as a verdict.
  for (const bad of [
    {},
    null,
    { contract_data: { val: { contract_instance: { executable: {} } } } },
    { contract_data: { val: { contract_instance: { executable: { wasm: 'nope' } } } } },
    { contract_data: { val: { contract_instance: { executable: { wasm: 'aa'.repeat(31) } } } } },
  ]) {
    assert.throws(() => executableFromEntryJson(bad), /unrecognised contract executable/);
  }
});
