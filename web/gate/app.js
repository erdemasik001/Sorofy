/**
 * Wiring. The three panels, in the order the product's argument runs:
 * check a contract, bring a balance, try to deposit it.
 *
 * Everything user-visible is written with `textContent`, never `innerHTML`. Contract ids,
 * anchor messages and error strings all originate outside this page, and the explorer holds the
 * same line for the same reason.
 */

import { ACTIVE_ASSET, ACTIVE_PROTOCOL, ACTIVE_REGISTRY, ANCHOR, ASSETS, NETWORK, PROTOCOLS } from './config.js';
import { checkContract, VERIFIED_IS_NOT_SAFE } from './lib/gate.js';
import { authenticate, depositStatus, ensureTrustline, fetchStellarToml, startInteractiveDeposit } from './lib/anchor.js';
import { supply } from './lib/protocol.js';
import { connect } from './lib/wallet.js';

const $ = (id) => document.getElementById(id);

const state = {
  wallet: null, // { address, signTransaction }
  anchor: null, // the parsed SEP-1 description
  token: null, // SEP-10 session
  asset: ASSETS[ACTIVE_ASSET],
  target: PROTOCOLS[ACTIVE_PROTOCOL],
  verdict: null,
};

/* ── rendering helpers ─────────────────────────────────────────────────────────────── */

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function fact(key, value, href) {
  const row = el('div', 'fact');
  row.append(el('span', 'fact__k', key));
  const v = el('span', 'fact__v');
  if (href) {
    const a = el('a', null, value);
    a.href = href;
    a.target = '_blank';
    a.rel = 'noreferrer noopener';
    v.append(a);
  } else {
    v.textContent = value;
  }
  row.append(v);
  return row;
}

const MARK = { ok: '✓', blocked: '✕', waiting: '…', danger: '!', unknown: '?' };

/** Draw a verdict into a panel. */
function renderVerdict(container, verdict, { extras = [], caveat = null } = {}) {
  container.hidden = false;
  container.dataset.tone = verdict.tone;
  container.replaceChildren();

  const head = el('div', 'verdict__head');
  head.append(el('span', 'verdict__mark', MARK[verdict.tone] ?? '?'));
  head.append(el('span', 'verdict__title', verdict.headline));
  if (verdict.state) head.append(el('span', 'verdict__state', verdict.state));
  container.append(head);

  const body = el('div', 'verdict__body');
  body.append(el('p', null, verdict.detail));

  const facts = el('div', 'facts');
  for (const [k, v, href] of extras) if (v) facts.append(fact(k, v, href));
  if (facts.children.length) body.append(facts);

  if (caveat) body.append(el('p', 'caveat', caveat));
  container.append(body);
}

function setStep(name, stateName, note) {
  const li = document.querySelector(`[data-step="${name}"]`);
  if (!li) return;
  li.dataset.state = stateName;
  li.querySelector('.steps__s').textContent = note ?? '';
}

function short(hash, head = 10, tail = 6) {
  return hash && hash.length > head + tail + 1 ? `${hash.slice(0, head)}…${hash.slice(-tail)}` : hash;
}

/* ── panel 1: the gate ─────────────────────────────────────────────────────────────── */

function targetOptions() {
  const select = $('target');
  select.replaceChildren();
  for (const [key, p] of Object.entries(PROTOCOLS)) {
    const option = el('option', null, p.label);
    option.value = key;
    select.append(option);
  }
  select.value = ACTIVE_PROTOCOL;
  select.addEventListener('change', () => {
    state.target = PROTOCOLS[select.value];
    state.verdict = null;
    $('verdict').hidden = true;
    describeTarget();
    refreshDepositButton();
  });
  describeTarget();
}

function describeTarget() {
  $('target-note').textContent = `${state.target.contractId} — ${state.target.note}`;
  $('pool-name').textContent = state.target.label;
}

async function runCheck(contractId) {
  const panel = $('verdict');
  panel.hidden = false;
  panel.dataset.tone = 'unknown';
  panel.replaceChildren(el('div', 'verdict__body', 'Reading the ledger…'));

  let verdict;
  try {
    verdict = await checkContract(contractId, { registryId: ACTIVE_REGISTRY });
  } catch (e) {
    verdict = {
      ok: false, allow: false, tone: 'unknown',
      headline: 'Cannot be checked',
      detail: e.message,
      claims: [],
    };
  }
  state.verdict = verdict;

  const extras = [
    ['Contract', contractId, `${NETWORK.explorer}/contract/${contractId}`],
    ['Deployed wasm', verdict.wasmHash ? short(verdict.wasmHash, 16, 8) : null],
    ['Registry', short(ACTIVE_REGISTRY, 8, 6), `${NETWORK.explorer}/contract/${ACTIVE_REGISTRY}`],
    ['Claims found', verdict.claims?.length ? String(verdict.claims.length) : 'none'],
  ];
  if (verdict.claim?.inputDigest) {
    extras.push(['Claim', short(verdict.claim.inputDigest, 16, 8)]);
    extras.push(['Discovered via', verdict.claim.source ?? 'chain events']);
  }

  renderVerdict(panel, verdict, {
    extras,
    caveat: verdict.allow ? VERIFIED_IS_NOT_SAFE : null,
  });
  refreshDepositButton();
}

/* ── panel 2: the anchor ───────────────────────────────────────────────────────────── */

function assetOptions() {
  const select = $('asset');
  select.replaceChildren();
  for (const [key, a] of Object.entries(ASSETS)) {
    const option = el('option', null, a.suppliable ? a.label : `${a.label} (deposit only)`);
    option.value = key;
    select.append(option);
  }
  select.value = ACTIVE_ASSET;
  select.addEventListener('change', () => {
    state.asset = ASSETS[select.value];
    refreshDepositButton();
  });
}

async function connectWallet() {
  setStep('wallet', 'active', 'waiting for the wallet…');
  try {
    state.wallet = await connect();
  } catch (e) {
    setStep('wallet', 'failed', e.message);
    return;
  }
  setStep('wallet', 'done', short(state.wallet.address, 6, 6));
  $('anchor-run').disabled = false;
  refreshDepositButton();
}

async function runAnchorFlow() {
  const note = $('anchor-note');
  $('anchor-run').disabled = true;

  try {
    // SEP-1
    setStep('toml', 'active', 'fetching…');
    state.anchor = await fetchStellarToml(ANCHOR.homeDomain);
    if (!state.anchor.usable) {
      setStep('toml', 'failed', `missing ${state.anchor.missing.join(', ')}`);
      return;
    }
    $('anchor-name').textContent = state.anchor.orgName ?? state.anchor.homeDomain;
    setStep('toml', 'done', `${state.anchor.currencies.length} assets, TRY: ${state.anchor.handlesTry ? 'yes' : 'no'}`);
    note.textContent = state.anchor.handlesTry
      ? 'This anchor lists TRY.'
      : 'This anchor does not handle TRY. What is demonstrated is the SEP-24 flow against a ' +
        'test anchor; a lira partner is an integration question, and calling this a lira ' +
        'on-ramp would be false.';

    // SEP-10
    setStep('auth', 'active', 'sign the challenge in your wallet…');
    state.token = await authenticate({
      anchor: state.anchor,
      account: state.wallet.address,
      signTransaction: state.wallet.signTransaction,
    });
    setStep('auth', 'done', 'session token held in this tab only');

    // Trustline
    setStep('trust', 'active', 'checking…');
    const trust = await ensureTrustline({
      asset: state.asset,
      account: state.wallet.address,
      signTransaction: state.wallet.signTransaction,
    });
    setStep('trust', trust.needed ? 'done' : 'skipped', trust.reason);

    // SEP-24
    setStep('deposit', 'active', 'opening the anchor’s own page…');
    const interactive = await startInteractiveDeposit({
      anchor: state.anchor,
      token: state.token,
      assetCode: state.asset.code,
      account: state.wallet.address,
    });
    // A popup, not an iframe: the anchor's KYC page is the anchor's, and it should be visibly
    // theirs. This page never sees an identity document.
    window.open(interactive.url, 'sorofy-anchor-deposit', 'width=480,height=720');
    await pollDeposit(interactive.id);
  } catch (e) {
    const active = document.querySelector('[data-state="active"]');
    if (active) setStep(active.dataset.step, 'failed', e.message);
    note.textContent = e.message;
  } finally {
    $('anchor-run').disabled = false;
  }
}

async function pollDeposit(id) {
  for (let i = 0; i < 240; i++) {
    const status = await depositStatus({ anchor: state.anchor, token: state.token, id });
    setStep('deposit', status.failed ? 'failed' : status.settled ? 'done' : 'active', status.status);
    if (status.settled) {
      $('anchor-note').textContent = status.amountOut
        ? `The anchor sent ${status.amountOut} ${state.asset.label}.`
        : 'The anchor reports the deposit as complete.';
      refreshDepositButton();
      return;
    }
    if (status.failed) {
      $('anchor-note').textContent = status.message ?? `The anchor reported "${status.status}".`;
      return;
    }
    await new Promise((r) => setTimeout(r, 3000));
  }
  setStep('deposit', 'active', 'still pending — the anchor has not settled it yet');
}

/* ── panel 3: the deposit ──────────────────────────────────────────────────────────── */

function refreshDepositButton() {
  const reasons = [];
  if (!state.wallet) reasons.push('connect a wallet');
  if (!state.verdict) reasons.push('check the target first');
  if (state.target.noDeposit) reasons.push(`${state.target.label} is not a pool`);
  if (state.target.unimplemented) reasons.push(`the ${state.target.kind} adapter is not wired up`);
  if (!state.asset.suppliable) reasons.push(`the pool has no ${state.asset.label} reserve`);

  $('deposit').disabled = reasons.length > 0;
  $('deposit-note').textContent = reasons.length
    ? `To supply: ${reasons.join('; ')}.`
    : state.verdict?.allow
      ? `Ready. The gate will be re-read against the pool’s current bytes before signing.`
      : `The gate currently says “${state.verdict.headline}”. Pressing Supply will be refused — ` +
        `which is the point, and worth doing on stage.`;
}

async function runDeposit() {
  const panel = $('deposit-result');
  panel.hidden = false;
  panel.dataset.tone = 'unknown';
  panel.replaceChildren(el('div', 'verdict__body', 'Re-reading consensus, then building…'));

  try {
    const result = await supply({
      pool: state.target,
      asset: state.asset,
      amount: $('amount').value,
      account: state.wallet.address,
      signTransaction: state.wallet.signTransaction,
      // The gate, re-read at the moment of the deposit rather than taken from the panel.
      recheck: async () => checkContract(state.target.contractId, { registryId: ACTIVE_REGISTRY }),
    });
    renderVerdict(
      panel,
      {
        tone: result.succeeded ? 'ok' : 'waiting',
        headline: result.succeeded ? 'Supplied' : `Submitted — ${result.status}`,
        detail: result.succeeded
          ? `${$('amount').value} ${state.asset.label} went into ${state.target.label}, after the ` +
            'gate re-read consensus and allowed it.'
          : 'The transaction was submitted; the network has not reported a final result yet.',
        state: result.verdict?.state,
      },
      { extras: [['Transaction', short(result.hash, 12, 8), `${NETWORK.explorer}/tx/${result.hash}`]] },
    );
  } catch (e) {
    if (e.blocked) {
      renderVerdict(panel, { ...e.verdict, headline: `Deposit refused — ${e.verdict.headline}` }, {
        extras: [['Target', state.target.contractId]],
        caveat:
          'This refusal is client-side (docs/anchor-integration.md §4a). It is a real check ' +
          'against real on-chain consensus, and it is not a guarantee: calling the pool ' +
          'directly bypasses it. Only the on-chain wrapper in §4b would bind.',
      });
      return;
    }
    renderVerdict(panel, {
      tone: 'danger', headline: 'Deposit failed', detail: e.message,
    });
  }
}

/* ── the limits, stated where the gate is shown ────────────────────────────────────── */

const LIMITS = [
  ['A verified contract is not a safe contract.', VERIFIED_IS_NOT_SAFE],
  [
    'This gate is client-side.',
    'It reads real consensus from the registry and refuses in earnest, but anyone can call the ' +
      'pool directly and nothing here stops them. The on-chain wrapper is a separate spike.',
  ],
  [
    'No lira.',
    'The test anchor handles SRT, USDC and XLM. The SEP-24 flow works end to end against it; a ' +
      'TRY partner is an integration question and is not demonstrated here.',
  ],
  [
    'Reading stays free.',
    'The check above needs no wallet, no account, no token and no payment, and it is the same ' +
      'answer anyone can read off the chain themselves. Money is downstream of verification, ' +
      'never a condition of it.',
  ],
  [
    'The gate is optional.',
    'It is a safer path, not a toll booth. Depositing directly into any pool works exactly as ' +
      'it did before, and Sorofy takes no fee from any of this.',
  ],
  [
    'Testnet.',
    'Every asset here is worthless, the verifier stake is a valueless token, and nothing on ' +
      'this page is money.',
  ],
];

function renderLimits() {
  const list = $('limits');
  for (const [head, body] of LIMITS) {
    const li = el('li');
    const span = el('span');
    span.append(el('strong', null, `${head} `));
    span.append(document.createTextNode(body));
    li.append(span);
    list.append(li);
  }
}

/* ── boot ──────────────────────────────────────────────────────────────────────────── */

targetOptions();
assetOptions();
renderLimits();
$('anchor-name').textContent = ANCHOR.homeDomain;

$('check').addEventListener('click', () => runCheck(state.target.contractId));
$('check-custom').addEventListener('click', () => {
  const id = $('custom').value.trim();
  if (id) runCheck(id);
});
$('custom').addEventListener('keydown', (e) => {
  if (e.key === 'Enter') $('check-custom').click();
});
$('connect').addEventListener('click', connectWallet);
$('anchor-run').addEventListener('click', runAnchorFlow);
$('deposit').addEventListener('click', runDeposit);

refreshDepositButton();
