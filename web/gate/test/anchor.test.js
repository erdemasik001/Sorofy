/**
 * SEP-1 parsing, against the anchor's real file.
 *
 * `fixtures.stellar.toml` was fetched from `https://testanchor.stellar.org/.well-known/stellar.toml`
 * on 2026-09-19. The assertions below are the ones the flow actually depends on, and one of
 * them records an inconvenient fact rather than working around it: this anchor does not handle
 * TRY, so nothing built on it may be described as a lira on-ramp.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

import { parseToml } from '../lib/toml.js';
import { describeAnchor } from '../lib/anchor.js';

const TOML_TEXT = readFileSync(new URL('./fixtures.stellar.toml', import.meta.url), 'utf8');
const anchor = () => describeAnchor(parseToml(TOML_TEXT), 'testanchor.stellar.org');

test('the anchor advertises everything the flow needs', () => {
  const a = anchor();
  assert.deepEqual(a.missing, []);
  assert.equal(a.usable, true);
  assert.equal(a.webAuth, 'https://testanchor.stellar.org/auth');
  assert.equal(a.sep24, 'https://testanchor.stellar.org/sep24');
  assert.equal(a.signingKey, 'GCHLHDBOKG2JWMJQBTLSL5XG6NO7ESXI2TAQKZXCXWXB5WI2X6W233PR');
  assert.equal(a.networkPassphrase, 'Test SDF Network ; September 2015');
  assert.equal(a.orgName, 'Stellar Development Foundation');
});

test('the anchor does not handle TRY, and the code says so out loud', () => {
  const a = anchor();
  assert.equal(a.handlesTry, false);
  assert.deepEqual(a.currencies.map((c) => c.code).sort(), ['SRT', 'USDC', 'native']);
});

test('the anchor’s USDC is not the pool’s USDC', () => {
  // Recorded because it is the kind of thing that gets assumed. The anchor issues USDC from
  // GBBD47IF…; the Blend v2 testnet pool's USDC reserve is a different issuer entirely, so one
  // cannot be supplied in place of the other. XLM is the asset the two ends share.
  const usdc = anchor().currencies.find((c) => c.code === 'USDC');
  assert.equal(usdc.issuer, 'GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5');
});

test('a file missing an endpoint is reported, not silently half-used', () => {
  const partial = describeAnchor(parseToml('SIGNING_KEY = "GABC"\n'), 'example.com');
  assert.equal(partial.usable, false);
  assert.deepEqual(partial.missing, ['WEB_AUTH_ENDPOINT (SEP-10)', 'TRANSFER_SERVER_SEP0024 (SEP-24)']);
});

test('the TOML subset covers what a stellar.toml actually contains', () => {
  const parsed = parseToml(TOML_TEXT);

  // Top-level scalars, an array of strings, array-of-tables, and a named table.
  assert.equal(parsed.VERSION, '0.1.0');
  assert.deepEqual(parsed.ACCOUNTS, ['GCSGSR6KQQ5BP2FXVPWRL6SWPUSFWLVONLIBJZUKTVQB5FYJFVL6XOXE']);
  assert.equal(parsed.CURRENCIES.length, 3);
  assert.equal(parsed.DOCUMENTATION.ORG_GITHUB, 'stellar');

  // Booleans are booleans, not the strings "false".
  assert.equal(parsed.CURRENCIES[0].is_asset_anchored, false);

  // A [[CURRENCIES]] entry must not leak into the one after it.
  assert.equal(parsed.CURRENCIES[2].code, 'native');
  assert.equal(parsed.CURRENCIES[2].issuer, undefined);
});

test('a # inside a description is not treated as a comment', () => {
  // `desc` fields are free text and routinely contain #; truncating one would corrupt the
  // value without any error.
  const parsed = parseToml('desc = "rate # 2 applies"\nother = 1 # a real comment\n');
  assert.equal(parsed.desc, 'rate # 2 applies');
  assert.equal(parsed.other, 1);
});

test('the parser skips what it cannot read rather than inventing a value', () => {
  const parsed = parseToml(
    ['GOOD = "yes"', 'nonsense line with no equals', '[TABLE]', 'INNER = 2'].join('\n'),
  );
  assert.equal(parsed.GOOD, 'yes');
  assert.equal(parsed.TABLE.INNER, 2);
  assert.equal(Object.keys(parsed).length, 2);
});
