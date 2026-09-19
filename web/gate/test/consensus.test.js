/**
 * The gate's decision, against values actually read off the chain.
 *
 * The fixtures are not invented. On 2026-09-19 the six claims the STEP 4 walk-through left on
 * the rehearsal registry `CDACBJDL…` were recovered from its `attested` events and each one's
 * `consensus()` was read back; those readings are what this file asserts against. So the
 * decision table is pinned to states the registry really produces, in the shape it really
 * produces them.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { normalizeConsensus, decide, bestOf, STATES, VERIFIED } from '../lib/consensus.js';

/** Read from CDACBJDL… on 2026-09-19, one per claim the walk-through created. */
const ON_CHAIN = {
  // claim 1 — a and b agreed with the chain, c did not
  disputed2of3: ['Disputed', { distinct: 2, top_count: 2, total: 3 }],
  // claim 2 — a, b and c all rebuilt the chain's hash, window closed
  verified: ['Verified'],
  // claim 4 — a alone, quorum is 3
  insufficient1: ['Insufficient', { distinct: 1, top_count: 1, total: 1 }],
  // claim 6 — a genuine 1–1
  insufficient1v1: ['Insufficient', { distinct: 2, top_count: 1, total: 2 }],
  // the live Blend v2 pool: nothing has ever been attested about it
  noClaim: ['NoClaim'],
};

test('exactly one state opens the gate', () => {
  const allowed = STATES.filter((state) => decide({ state, tally: null }).allow);
  assert.deepEqual(allowed, [VERIFIED]);
});

test('every state the registry defines produces a decision with a reason', () => {
  for (const state of STATES) {
    const d = decide({ state, tally: { distinct: 2, top: 1, total: 3 } });
    assert.equal(typeof d.headline, 'string');
    assert.ok(d.headline.length > 0, `${state} has no headline`);
    assert.ok(d.detail.length > 40, `${state} has no usable explanation`);
    assert.ok(['ok', 'blocked', 'waiting', 'danger'].includes(d.tone), `${state} tone`);
  }
});

test('the real on-chain readings decide the way the evidence says they should', () => {
  const verdict = (raw) => decide(normalizeConsensus(raw));

  assert.equal(verdict(ON_CHAIN.verified).allow, true);

  for (const [name, raw] of Object.entries(ON_CHAIN)) {
    if (name === 'verified') continue;
    assert.equal(verdict(raw).allow, false, `${name} must not open the gate`);
  }

  // A dispute is not "pending" — it is a red flag, and it reads as one.
  assert.equal(verdict(ON_CHAIN.disputed2of3).tone, 'danger');
  // An empty registry is an absence of evidence, not an accusation.
  assert.match(verdict(ON_CHAIN.noClaim).detail, /absence of evidence/);
  // The 1–1 split says how many looked, so "2" has to reach the text.
  assert.match(verdict(ON_CHAIN.insufficient1v1).detail, /\b2\b/);
});

test('the three decoded shapes all normalize to the same thing', () => {
  // Array (what scValToNative gives), bare string, and the object form JSON round-trips to.
  assert.deepEqual(normalizeConsensus(['Verified']), { state: 'Verified', tally: null });
  assert.deepEqual(normalizeConsensus('Verified'), { state: 'Verified', tally: null });

  const expected = { state: 'Disputed', tally: { distinct: 2, top: 2, total: 3 } };
  assert.deepEqual(normalizeConsensus(['Disputed', { distinct: 2, top_count: 2, total: 3 }]), expected);
  assert.deepEqual(normalizeConsensus({ Disputed: { distinct: 2, top_count: 2, total: 3 } }), expected);
});

test('u32 counts that arrive as BigInt are still numbers by the time they are rendered', () => {
  // The SDK decodes some integers as BigInt; `${bigint}` works but arithmetic against a number
  // throws, so this is converted once at the boundary rather than guarded at every use.
  const { tally } = normalizeConsensus(['Disputed', { distinct: 2n, top_count: 2n, total: 3n }]);
  assert.deepEqual(tally, { distinct: 2, top: 2, total: 3 });
});

test('an unrecognised answer throws instead of quietly reading as "not verified"', () => {
  // A block that comes from a decoding bug looks exactly like a legitimate block, which is the
  // worst possible failure: the gate would be reporting on itself, not on the contract.
  assert.throws(() => normalizeConsensus(['Verifed']), /unknown consensus state/);
  assert.throws(() => normalizeConsensus(null), /unrecognised consensus value/);
  assert.throws(() => normalizeConsensus(42), /unrecognised consensus value/);
  assert.throws(() => normalizeConsensus({ a: 1, b: 2 }), /unrecognised consensus value/);
  assert.throws(() => decide({ state: 'Elsewhere', tally: null }), /no decision defined/);
});

test('with several claims for one hash, one Verified claim is enough', () => {
  const claims = [
    { ...normalizeConsensus(ON_CHAIN.noClaim), claim: 'a' },
    { ...normalizeConsensus(ON_CHAIN.verified), claim: 'b' },
    { ...normalizeConsensus(ON_CHAIN.insufficient1), claim: 'c' },
  ];
  const best = bestOf(claims);
  assert.equal(best.allow, true);
  assert.equal(best.claim, 'b', 'the decision must name which claim carried it');
});

test('without a Verified claim, the most informative state is the one shown', () => {
  const best = bestOf([
    { ...normalizeConsensus(ON_CHAIN.noClaim), claim: 'a' },
    { ...normalizeConsensus(ON_CHAIN.disputed2of3), claim: 'b' },
  ]);
  assert.equal(best.allow, false);
  // "we looked and they disagreed" outranks "some other claim has nothing on it".
  assert.equal(best.state, 'Disputed');
  assert.equal(best.tally.total, 3);
});

test('no claims at all reads as NoClaim rather than as an error or a pass', () => {
  for (const empty of [[], null, undefined]) {
    const best = bestOf(empty);
    assert.equal(best.allow, false);
    assert.equal(best.state, 'NoClaim');
    assert.equal(best.claim, null);
  }
});
