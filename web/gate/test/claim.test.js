/**
 * The gate's claim digest against the contract's own vectors.
 *
 * These are the same four expected values that `contracts/registry/src/test.rs` pins the
 * registry to, and that `crates/api/src/claim.rs` pins the API to. Their provenance is a Python
 * implementation of the layout written independently of any of the three, so agreeing with them
 * means agreeing with a third party rather than with one author's reading.
 *
 *   node --test web/gate/test/
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { inputDigestHex, imageDigestHex, fromHex, toHex } from '../lib/claim.js';

const SRC = '11'.repeat(32);
const IMG = '22'.repeat(32);

test('the claim digest matches the registry’s reference vectors', async () => {
  assert.equal(
    await inputDigestHex({
      sourceSha256: SRC,
      bldimgDigest: IMG,
      bldopt: ['--package=vrfy-token', '--optimize'],
    }),
    'ded0494ae3e75310e4989acb8daacfa79636c7e3c211b1af884fa2376ed38381',
  );

  assert.equal(
    await inputDigestHex({ sourceSha256: SRC, bldimgDigest: IMG, bldopt: [] }),
    '14ee749e3747cf362fffeb845000c5395c9d05055bb8f7e2fb48d41ce7e5064e',
  );

  // Flag order is part of the question.
  assert.equal(
    await inputDigestHex({
      sourceSha256: SRC,
      bldimgDigest: IMG,
      bldopt: ['--optimize', '--package=vrfy-token'],
    }),
    'c0823d00b2b99e2f425b9f3fa283f652e315cca163ef3ad10e7fa1f408d881fb',
  );
});

test('the claim digest of the real VRFY token build', async () => {
  // The staged-tree and image digests Sorofy's engine reported when it rebuilt the deployed
  // token, with the one flag it was built with.
  assert.equal(
    await inputDigestHex({
      sourceSha256: '47a88a77289c02370d0f04995e6faadcea4f5fb081981ff5ee48762804c0f306',
      bldimgDigest: 'cff44167d2ee90f901c768ccdb75eb5c382c95295f44bcd7ee4b543f0adf9588',
      bldopt: ['--package=vrfy-token'],
    }),
    '32f0a6e512a0259f9d4e2f5a5781cfda2af1782cf4246d9a0fa9a1f314d262fb',
  );
});

test('an omitted bldopt list is the same as an empty one', async () => {
  assert.equal(
    await inputDigestHex({ sourceSha256: SRC, bldimgDigest: IMG }),
    await inputDigestHex({ sourceSha256: SRC, bldimgDigest: IMG, bldopt: [] }),
  );
});

test('length prefixes separate flag lists that would otherwise concatenate the same', async () => {
  const a = await inputDigestHex({ sourceSha256: SRC, bldimgDigest: IMG, bldopt: ['ab', 'c'] });
  const b = await inputDigestHex({ sourceSha256: SRC, bldimgDigest: IMG, bldopt: ['a', 'bc'] });
  assert.notEqual(a, b);
});

test('flags are hashed as UTF-8, so a non-ASCII flag is not silently mangled', async () => {
  // `TextEncoder` is the only encoder used, matching Rust's `str::as_bytes`. If this ever
  // changed to a per-char byte, a flag carrying a non-ASCII path would file a different claim.
  const digest = await inputDigestHex({
    sourceSha256: SRC,
    bldimgDigest: IMG,
    bldopt: ['--path=/tmp/é'],
  });
  assert.match(digest, /^[0-9a-f]{64}$/);
  assert.notEqual(
    digest,
    await inputDigestHex({ sourceSha256: SRC, bldimgDigest: IMG, bldopt: ['--path=/tmp/e'] }),
  );
});

test('a malformed digest is refused rather than padded or truncated', async () => {
  const bad = [
    { sourceSha256: 'aa'.repeat(31), bldimgDigest: IMG },
    { sourceSha256: 'aa'.repeat(33), bldimgDigest: IMG },
    { sourceSha256: SRC, bldimgDigest: 'zz'.repeat(32) },
    { sourceSha256: SRC, bldimgDigest: 'abc' },
  ];
  for (const descriptor of bad) {
    await assert.rejects(() => inputDigestHex(descriptor));
  }
});

test('the image digest is taken from a pinned reference and nothing else', () => {
  assert.equal(
    imageDigestHex(
      'ghcr.io/erdemasik001/sorofy-build-image@sha256:' +
        'cff44167d2ee90f901c768ccdb75eb5c382c95295f44bcd7ee4b543f0adf9588',
    ),
    'cff44167d2ee90f901c768ccdb75eb5c382c95295f44bcd7ee4b543f0adf9588',
  );
  // A bare digest is accepted; a movable tag is not, because it names no fixed image.
  assert.equal(imageDigestHex('CFF44167'.repeat(8)), 'cff44167'.repeat(8));
  assert.throws(() => imageDigestHex('ghcr.io/x/y:latest'), /not pinned by digest/);
});

test('hex round-trips and rejects odd input', () => {
  assert.equal(toHex(fromHex('DEADBEEF')), 'deadbeef');
  assert.deepEqual(Array.from(fromHex('00ff')), [0, 255]);
  assert.throws(() => fromHex('abc'), /not hex/);
  assert.throws(() => fromHex('gg'), /not hex/);
});
