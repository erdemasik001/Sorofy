/**
 * Everything the gate reads from the chain.
 *
 * Two properties this file exists to preserve:
 *
 * 1. **The chain is the only authority.** The wasm hash comes from the contract's own ledger
 *    entry and the verdict comes from the registry. Neither is taken from Sorofy's API, which
 *    means a lying or compromised API cannot turn a block into a pass — at worst it names the
 *    wrong claim, and a claim nobody attested reads as `NoClaim`, which blocks.
 *
 * 2. **Reading is free, anonymous and token-free** (docs/anchor-integration.md §8). Every
 *    function here works with no wallet connected, no account, no key and no API token: the
 *    simulation source is a keypair generated in the page and thrown away, never funded and
 *    never used to sign. You can ask whether a contract is verified without identifying
 *    yourself to anyone.
 *
 * The SDK import is deliberately lazy, and the event scan does not use the SDK at all, so the
 * discovery path can be unit-tested in Node against captured bytes.
 */

import { NETWORK, EVENT_SCAN, VENDOR } from '../config.js';
import { fromHex, toHex } from './claim.js';
import { normalizeConsensus } from './consensus.js';
import { decodeScVal } from './xdr.js';

let sdkPromise = null;

/** The pinned Stellar SDK, imported once, on first use. */
export function loadSdk() {
  if (!sdkPromise) {
    sdkPromise = import(/* @vite-ignore */ VENDOR.stellarSdk).catch((e) => {
      sdkPromise = null;
      throw new Error(`could not load the Stellar SDK from ${VENDOR.stellarSdk}: ${e.message}`);
    });
  }
  return sdkPromise;
}

/* ── raw JSON-RPC ─────────────────────────────────────────────────────────────────────── */

let rpcId = 0;

/** One JSON-RPC call against the configured Soroban RPC. */
export async function rpc(method, params) {
  const res = await fetch(NETWORK.rpc, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', id: ++rpcId, method, params }),
  });
  if (!res.ok) throw new Error(`RPC ${method}: HTTP ${res.status}`);
  const body = await res.json();
  if (body.error) {
    throw new Error(`RPC ${method}: ${body.error.message ?? JSON.stringify(body.error)}`);
  }
  return body.result;
}

/* ── pure helpers, unit-tested against captured bytes ─────────────────────────────────── */

/**
 * The base64 XDR of `ScVal::Bytes(32)` — the shape a digest takes as an event topic.
 *
 * Hand-encoded so the event scan needs no SDK, and so the encoding is a pure function with a
 * test rather than a trusted library call. XDR's layout: a 4-byte discriminant (13 = SCV_BYTES),
 * a 4-byte length, then the bytes, padded to a multiple of four — which 32 already is.
 */
export function bytesScValBase64(hex) {
  const bytes = fromHex(hex, 32);
  const out = new Uint8Array(8 + 32);
  const view = new DataView(out.buffer);
  view.setUint32(0, 13, false);
  view.setUint32(4, 32, false);
  out.set(bytes, 8);
  let bin = '';
  for (const b of out) bin += String.fromCharCode(b);
  return btoa(bin);
}

/**
 * The ledger a `getEvents` cursor points just past.
 *
 * Cursors are `<toid>-<index>` and the ledger is the TOID's top 32 bits. The scan needs this
 * because the RPC returns a cursor on every page including empty ones: a wide window comes back
 * as several empty pages before the page that actually holds the events, so stopping at the
 * first empty page silently finds nothing. Observed on 2026-09-19 — a 16 000-ledger window
 * returned zero events where an 8 000-ledger one returned 21.
 */
export function cursorLedger(cursor) {
  if (typeof cursor !== 'string') return null;
  const toid = cursor.split('-')[0];
  if (!/^\d+$/.test(toid)) return null;
  return Number(BigInt(toid) >> 32n);
}

/**
 * The claim one registry event records, or `null` if it is not an attestation.
 *
 * The registry publishes `attested` with the verifier and the wasm hash as topics and the rest
 * in the value, so topic 2 is the hash and `input_digest` completes the claim.
 */
export function claimFromEvent(event) {
  const topics = event.topic ?? event.topics ?? [];
  if (topics.length < 3) return null;
  if (decodeScVal(topics[0]) !== 'attested') return null;
  const wasmHash = decodeScVal(topics[2]);
  const value = decodeScVal(event.value);
  const inputDigest = value?.input_digest;
  if (typeof wasmHash !== 'string' || typeof inputDigest !== 'string') return null;
  return {
    wasmHash,
    inputDigest,
    rebuiltHash: value.rebuilt_hash ?? null,
    verifier: decodeScVal(topics[1])?.key ?? null,
    ledger: event.ledger,
  };
}

/** Distinct claims for one wasm hash, from a page of events. */
export function claimsForHash(events, wasmHashHex) {
  const want = String(wasmHashHex).toLowerCase();
  const seen = new Map();
  for (const event of events ?? []) {
    let claim;
    try {
      claim = claimFromEvent(event);
    } catch {
      // An event this reader cannot parse is not evidence of anything; skip it rather than
      // failing the whole scan over one unexpected shape.
      continue;
    }
    if (claim && claim.wasmHash === want && !seen.has(claim.inputDigest)) {
      seen.set(claim.inputDigest, claim);
    }
  }
  return [...seen.values()];
}

/* ── reads ────────────────────────────────────────────────────────────────────────────── */

/**
 * Read a decoded contract-instance ledger entry and say what it executes.
 *
 * The input is the canonical JSON an `xdr.LedgerEntryData` serializes to — a stable, readable
 * shape, unlike the SDK's accessor objects, whose method names are minified and have moved
 * between versions. Both forms the executable takes were captured from the live network:
 * `{ wasm: "<64 hex>" }` for a deployed contract, and the bare string `"stellar_asset"` for a
 * built-in Stellar Asset Contract.
 *
 * Anything else throws. This value is the subject of every other judgement the gate makes, so
 * guessing at an unfamiliar shape would put a wrong hash in front of the registry — and a hash
 * nobody attested reads as `NoClaim`, which would look like a verdict rather than a bug.
 */
export function executableFromEntryJson(json) {
  const exe = json?.contract_data?.val?.contract_instance?.executable;
  if (exe === 'stellar_asset' || (exe && typeof exe === 'object' && 'stellar_asset' in exe)) {
    return { stellarAsset: true };
  }
  const wasm = exe && typeof exe === 'object' ? exe.wasm : null;
  if (typeof wasm === 'string' && /^[0-9a-f]{64}$/i.test(wasm)) {
    return { wasmHash: wasm.toLowerCase() };
  }
  throw new Error(`unrecognised contract executable: ${JSON.stringify(exe)?.slice(0, 80)}`);
}

/**
 * What `contractId` executes on chain: `{ wasmHash }`, `{ stellarAsset: true }`, or `null` if
 * no such contract exists on this network.
 *
 * This is the gate's trust anchor — the same `getLedgerEntries` lookup on the contract's
 * instance entry that `crates/api/src/rpc.rs` performs server-side, done here so the browser
 * does not have to take anyone's word for which bytes it is about to be asked to fund.
 *
 * The request is made directly rather than through `Server.getContractData`, whose result shape
 * differs across SDK builds; `LedgerEntryData.fromXDR` is the part of the SDK worth depending
 * on, and it is the same decode `rpc.rs` performs.
 */
export async function executableOf(contractId) {
  const { Contract, xdr } = await loadSdk();
  const key = new Contract(contractId).getFootprint().toXDR('base64');
  const result = await rpc('getLedgerEntries', { keys: [key] });
  const entry = result?.entries?.[0];
  // A well-formed question about an absent entry, which is not a failure.
  if (!entry?.xdr) return null;
  return executableFromEntryJson(
    JSON.parse(JSON.stringify(xdr.LedgerEntryData.fromXDR(entry.xdr, 'base64'))),
  );
}

/**
 * `consensus(wasm_hash, input_digest)` on the registry.
 *
 * Simulated, never submitted: it is a read, it costs nothing, it writes nothing, and the
 * account it is simulated from does not need to exist — checked against the live network rather
 * than assumed.
 */
export async function readConsensus(registryId, wasmHashHex, inputDigestHex) {
  const sdk = await loadSdk();
  const { rpc: rpcNs, xdr, Contract, TransactionBuilder, Account, Keypair, scValToNative, BASE_FEE } =
    sdk;
  const server = new rpcNs.Server(NETWORK.rpc);

  const call = new Contract(registryId).call(
    'consensus',
    xdr.ScVal.scvBytes(bytesFor(sdk, wasmHashHex)),
    xdr.ScVal.scvBytes(bytesFor(sdk, inputDigestHex)),
  );
  // A throwaway identity, generated in the page and never funded or used to sign. Simulation
  // does not check that the source exists, so reading stays anonymous.
  const source = new Account(Keypair.random().publicKey(), '0');
  const tx = new TransactionBuilder(source, { fee: BASE_FEE, networkPassphrase: NETWORK.passphrase })
    .addOperation(call)
    .setTimeout(30)
    .build();

  const sim = await server.simulateTransaction(tx);
  if (rpcNs.Api.isSimulationError(sim)) throw new Error(`consensus() failed: ${sim.error}`);
  return normalizeConsensus(scValToNative(sim.result.retval));
}

/** A 32-byte value as whichever array type the loaded SDK build expects. */
function bytesFor(sdk, hex) {
  const bytes = fromHex(hex, 32);
  return typeof sdk.Buffer === 'function' ? sdk.Buffer.from(bytes) : bytes;
}

/**
 * Every claim the registry has seen for `wasmHashHex`, discovered from its own events.
 *
 * This is how the gate answers "is there *any* verified claim for these bytes" against a
 * registry that only offers `consensus(wasm_hash, input_digest)` — the gap
 * docs/anchor-integration.md §4b identifies. The filter is applied by the RPC, since topic 2 of
 * an `attested` event is the wasm hash, so only relevant events cross the wire.
 *
 * It is bounded by what the RPC still holds: ~121 000 ledgers, about a week, on 2026-09-19. An
 * attestation older than that window cannot be discovered this way, which is what the
 * configured build descriptors in config.js are for.
 */
export async function discoverClaims(registryId, wasmHashHex) {
  const { sequence: latest } = await rpc('getLatestLedger', {});
  const topic = bytesScValBase64(wasmHashHex);
  const filters = [{ type: 'contract', contractIds: [registryId], topics: [['*', '*', topic]] }];

  const found = new Map();
  let cursor = null;
  let startLedger = Math.max(1, latest - EVENT_SCAN.maxLedgersBack);

  for (let page = 0; page < EVENT_SCAN.maxPages; page++) {
    const params = { filters, pagination: { limit: EVENT_SCAN.pageLimit } };
    if (cursor) params.pagination.cursor = cursor;
    else params.startLedger = startLedger;

    let result;
    try {
      result = await rpc('getEvents', params);
    } catch (e) {
      // Asking before the RPC's retention window is an error that names the oldest ledger it
      // does hold; retry once from there. Otherwise stop — a scan that cannot run is not a
      // verdict, and the descriptor path still has a chance to name the claim.
      const oldest = Number(/\b(\d{6,})\b/.exec(e.message ?? '')?.[1]);
      if (!cursor && Number.isFinite(oldest) && oldest > startLedger) {
        startLedger = oldest;
        continue;
      }
      break;
    }

    for (const claim of claimsForHash(result.events, wasmHashHex)) {
      if (!found.has(claim.inputDigest)) found.set(claim.inputDigest, claim);
    }

    cursor = result.cursor;
    const at = cursorLedger(cursor);
    if (!cursor || (at !== null && at >= (result.latestLedger ?? latest))) break;
  }
  return [...found.values()];
}
