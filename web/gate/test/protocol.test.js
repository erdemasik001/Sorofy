/**
 * The gate as a *control*, not a display, and the amount arithmetic under it.
 *
 * `supply` is the only function in this project that moves money, so the property worth pinning
 * is that it refuses on its own — before any transaction is built — rather than relying on a
 * button being disabled. The SDK is never reached in these tests because the refusal happens
 * first, which is itself the assertion.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { supply, toStroops, fromStroops, REQUEST_TYPE } from '../lib/protocol.js';
import { decide } from '../lib/consensus.js';

const POOL = { contractId: 'CCEBVDYM32YNYCVNRXQKDFFPISJJCV557CDZEIRBEE4NCV4KHPQ44HGF' };
const XLM = { code: 'native', label: 'XLM', sac: 'CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC', decimals: 7 };
const ACCOUNT = 'GAUNIN5OOJJDWPLCIINMXHKUKIAUGQZZ3YQMWOTHULFB2VFVOPSIDCHD';

/** A signer that fails the test if it is ever reached. */
const refuseToSign = () => {
  throw new Error('the wallet must not be asked to sign a blocked deposit');
};

test('a deposit into an unverified contract is refused before anything is built', async () => {
  for (const state of ['NoClaim', 'Open', 'Insufficient', 'Mismatch', 'Disputed']) {
    const verdict = { ...decide({ state, tally: { distinct: 2, top: 1, total: 3 } }), state };
    await assert.rejects(
      () =>
        supply({
          pool: POOL,
          asset: XLM,
          amount: '5',
          account: ACCOUNT,
          signTransaction: refuseToSign,
          recheck: async () => verdict,
        }),
      (e) => {
        assert.equal(e.blocked, true, `${state} should be reported as a block`);
        assert.equal(e.verdict.state, state, 'the error carries which state blocked it');
        assert.match(e.message, /blocked by the verification gate/);
        return true;
      },
      `${state} must not reach the wallet`,
    );
  }
});

test('the gate is consulted at deposit time, not taken from the panel', async () => {
  // The interface may have shown Verified a minute ago. What matters is the answer now, so
  // `recheck` has to actually be called on every attempt.
  let calls = 0;
  const recheck = async () => {
    calls++;
    return { ...decide({ state: 'NoClaim', tally: null }), state: 'NoClaim' };
  };
  for (let i = 0; i < 3; i++) {
    await assert.rejects(() =>
      supply({ pool: POOL, asset: XLM, amount: '1', account: ACCOUNT, signTransaction: refuseToSign, recheck }),
    );
  }
  assert.equal(calls, 3, 'every attempt must re-read consensus');
});

test('Supply is request type 0, as the pool contract defines it', () => {
  // From blend-contracts-v2/pool/src/pool/actions.rs. A wrong number here would not error —
  // it would perform a different action with the user's money.
  assert.equal(REQUEST_TYPE.Supply, 0);
  assert.equal(REQUEST_TYPE.SupplyCollateral, 2);
});

test('amounts convert to the smallest unit without floating point', () => {
  assert.equal(toStroops('1', 7), 10_000_000n);
  assert.equal(toStroops('0.0000001', 7), 1n);
  assert.equal(toStroops('123.456', 7), 1_234_560_000n);
  // The case that makes this worth doing with strings: 0.1 + 0.2 is not 0.3 in a float, and
  // an amount is not a quantity you may approximate.
  assert.equal(toStroops('0.3', 7), 3_000_000n);
  assert.equal(toStroops('10000000.0000001', 7), 100_000_000_000_001n);
});

test('an amount that cannot be represented exactly is refused, not rounded', () => {
  assert.throws(() => toStroops('0.00000001', 7), /more than 7 decimal places/);
  for (const bad of ['', '-1', 'abc', '1e5', '0', '0.0', '1.2.3', ' ']) {
    assert.throws(() => toStroops(bad, 7), undefined, `${JSON.stringify(bad)} should be refused`);
  }
});

test('amounts round-trip back to what the user typed', () => {
  // Compared against the string the user typed, not against `Number(...)`: 0.0000001 through
  // a float comes back as "1e-7", which is exactly the rendering an amount field must not do.
  for (const amount of ['1', '0.0000001', '123.456', '0.3', '999999']) {
    assert.equal(fromStroops(toStroops(amount, 7), 7), amount);
  }
  assert.equal(fromStroops(0n, 7), '0');
  assert.equal(fromStroops(10_000_000n, 7), '1');
});
