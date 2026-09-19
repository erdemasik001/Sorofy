/**
 * The anchor half: SEP-1 discovery, SEP-10 authentication, SEP-24 interactive deposit.
 *
 * No contract is written for any of this — it is classic-Stellar front-end work, exactly as
 * docs/anchor-integration.md §5 describes. What it produces is a *balance*, which is the input
 * the gate then decides whether to let into a protocol.
 *
 * The one place this file is opinionated is SEP-10. A challenge is a transaction a remote
 * server hands you and asks you to sign; signing whatever arrives is how a wallet gets drained.
 * `authenticate` therefore validates the challenge against the anchor's own signing key from
 * its SEP-1 file, and against the account it was requested for, **before** the wallet is ever
 * asked — and it refuses rather than warns.
 */

import { ANCHOR, NETWORK } from '../config.js';
import { parseToml } from './toml.js';
import { loadSdk } from './chain.js';

/** SEP-1: the anchor's `stellar.toml`, parsed and checked for what the flow needs. */
export async function fetchStellarToml(homeDomain = ANCHOR.homeDomain) {
  const url = `https://${homeDomain}/.well-known/stellar.toml`;
  // No request headers, deliberately. An `Accept: text/plain` here is enough to make the
  // browser preflight, and the anchor answers the preflight without an `Access-Control-Allow-
  // Origin`, so the fetch fails — while the plain GET it would have made succeeds. Observed
  // against testanchor.stellar.org on 2026-09-19; the endpoint returns text/plain regardless.
  const res = await fetch(url);
  if (!res.ok) throw new Error(`SEP-1: ${url} returned HTTP ${res.status}`);
  return describeAnchor(parseToml(await res.text()), homeDomain);
}

/**
 * Turn a parsed SEP-1 file into what the flow needs, or say what is missing.
 *
 * Pure, so the shape of a real anchor's file can be pinned by a test.
 */
export function describeAnchor(toml, homeDomain) {
  const missing = [];
  const webAuth = toml.WEB_AUTH_ENDPOINT;
  const sep24 = toml.TRANSFER_SERVER_SEP0024;
  const signingKey = toml.SIGNING_KEY;
  if (!webAuth) missing.push('WEB_AUTH_ENDPOINT (SEP-10)');
  if (!sep24) missing.push('TRANSFER_SERVER_SEP0024 (SEP-24)');
  if (!signingKey) missing.push('SIGNING_KEY');

  const currencies = (toml.CURRENCIES ?? []).map((c) => ({
    code: c.code,
    issuer: c.issuer ?? null,
    desc: c.desc ?? '',
  }));

  return {
    homeDomain,
    webAuth,
    sep24,
    signingKey,
    kycServer: toml.KYC_SERVER ?? null,
    networkPassphrase: toml.NETWORK_PASSPHRASE ?? null,
    currencies,
    orgName: toml.DOCUMENTATION?.ORG_NAME ?? null,
    missing,
    usable: missing.length === 0,
    /**
     * Whether this anchor handles Turkish lira.
     *
     * Asked explicitly rather than assumed either way. On the SDF test anchor the answer is no
     * — its currencies are SRT, USDC and native — and docs/anchor-integration.md §9 fixes the
     * wording that follows from that: the flow works end to end against a test anchor; a TRY
     * partner is an integration question. Not "lira on-ramp".
     */
    handlesTry: currencies.some((c) => /^TRY$/i.test(c.code ?? '')),
  };
}

/** SEP-24 `/info`: which assets this anchor will actually accept, and the limits. */
export async function fetchDepositInfo(sep24Url) {
  const res = await fetch(`${sep24Url}/info`);
  if (!res.ok) throw new Error(`SEP-24 /info returned HTTP ${res.status}`);
  const body = await res.json();
  return Object.entries(body.deposit ?? {})
    .filter(([, v]) => v.enabled)
    .map(([code, v]) => ({ code, min: v.min_amount ?? null, max: v.max_amount ?? null }));
}

/**
 * SEP-10: sign the anchor's challenge and exchange it for a session token.
 *
 * The challenge is read and validated by the SDK's own `WebAuth.readChallengeTx`, which checks
 * the anchor's signature, the home domain, the web-auth domain, the sequence number and the
 * operation shape. If any of that fails this throws and the wallet is never asked to sign —
 * the whole risk of SEP-10 is being talked into signing something that is not a challenge.
 */
export async function authenticate({ anchor, account, signTransaction }) {
  const { WebAuth } = await loadSdk();

  const url = new URL(anchor.webAuth);
  url.searchParams.set('account', account);
  url.searchParams.set('home_domain', anchor.homeDomain);

  const res = await fetch(url);
  if (!res.ok) throw new Error(`SEP-10 challenge: HTTP ${res.status} from ${anchor.webAuth}`);
  const { transaction, network_passphrase: passphrase } = await res.json();
  if (!transaction) throw new Error('SEP-10: the anchor returned no challenge transaction');

  // The anchor states its own network in SEP-1; if it disagrees with ours, stop. Signing a
  // challenge built for another network is how a testnet signature becomes a mainnet one.
  const network = passphrase ?? anchor.networkPassphrase ?? NETWORK.passphrase;
  if (network !== NETWORK.passphrase) {
    throw new Error(`SEP-10: anchor is on "${network}", this page is on "${NETWORK.passphrase}"`);
  }

  const webAuthDomain = new URL(anchor.webAuth).hostname;
  const challenge = WebAuth.readChallengeTx(
    transaction,
    anchor.signingKey,
    network,
    anchor.homeDomain,
    webAuthDomain,
  );
  // `readChallengeTx` proves the anchor built and signed it. This proves it is *ours*: a
  // challenge issued for a different account must not be signed by this one.
  if (challenge.clientAccountID !== account) {
    throw new Error(
      `SEP-10: challenge is addressed to ${challenge.clientAccountID}, not ${account}`,
    );
  }

  const signedXdr = await signTransaction(transaction, { networkPassphrase: network });
  const post = await fetch(anchor.webAuth, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ transaction: signedXdr }),
  });
  if (!post.ok) {
    throw new Error(`SEP-10 token: HTTP ${post.status} — ${(await post.text()).slice(0, 200)}`);
  }
  const { token } = await post.json();
  if (!token) throw new Error('SEP-10: the anchor returned no token');
  return token;
}

/**
 * SEP-24: ask the anchor to open its own deposit UI.
 *
 * KYC, bank details and the amount all happen inside the anchor's page (SEP-12), not this one.
 * That division is the point: the gate never sees identity documents, and the anchor never
 * sees anything about verification.
 */
export async function startInteractiveDeposit({ anchor, token, assetCode, account }) {
  const res = await fetch(`${anchor.sep24}/transactions/deposit/interactive`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}` },
    body: JSON.stringify({ asset_code: assetCode, account }),
  });
  if (!res.ok) {
    throw new Error(`SEP-24 deposit: HTTP ${res.status} — ${(await res.text()).slice(0, 200)}`);
  }
  const body = await res.json();
  if (!body.url || !body.id) throw new Error('SEP-24: the anchor returned no interactive URL');
  return { url: body.url, id: body.id };
}

/** SEP-24: one poll of a transaction's status. */
export async function depositStatus({ anchor, token, id }) {
  const res = await fetch(`${anchor.sep24}/transaction?id=${encodeURIComponent(id)}`, {
    headers: { Authorization: `Bearer ${token}` },
  });
  if (!res.ok) throw new Error(`SEP-24 status: HTTP ${res.status}`);
  const { transaction } = await res.json();
  return {
    status: transaction?.status ?? 'unknown',
    amountOut: transaction?.amount_out ?? null,
    message: transaction?.message ?? null,
    settled: SETTLED.has(transaction?.status),
    failed: FAILED.has(transaction?.status),
  };
}

const SETTLED = new Set(['completed']);
const FAILED = new Set(['error', 'refunded', 'expired', 'no_market', 'too_small', 'too_large']);

/**
 * Ensure the account can hold `asset`, creating the trustline if it cannot.
 *
 * Returns what it did, because "nothing" is a real and common answer: **XLM is native and needs
 * no trustline at all**, which is worth saying rather than hiding behind a no-op. The step is
 * implemented in full for the issued assets (SRT, USDC) where it genuinely is required.
 */
export async function ensureTrustline({ asset, account, signTransaction }) {
  if (!asset.issuer) {
    return { needed: false, created: false, reason: `${asset.label} is the native asset` };
  }
  const { Horizon, Asset, Operation, TransactionBuilder, BASE_FEE } = await loadSdk();
  const server = new Horizon.Server(NETWORK.horizon);

  const loaded = await server.loadAccount(account);
  const already = loaded.balances.some(
    (b) => b.asset_code === asset.code && b.asset_issuer === asset.issuer,
  );
  if (already) {
    return { needed: true, created: false, reason: `a trustline to ${asset.code} already exists` };
  }

  const tx = new TransactionBuilder(loaded, { fee: BASE_FEE, networkPassphrase: NETWORK.passphrase })
    .addOperation(Operation.changeTrust({ asset: new Asset(asset.code, asset.issuer) }))
    .setTimeout(120)
    .build();

  const signedXdr = await signTransaction(tx.toXDR(), { networkPassphrase: NETWORK.passphrase });
  const submitted = await server.submitTransaction(
    TransactionBuilder.fromXDR(signedXdr, NETWORK.passphrase),
  );
  return { needed: true, created: true, hash: submitted.hash, reason: `trustline to ${asset.code} created` };
}
