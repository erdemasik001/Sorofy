/**
 * The protocol half: supplying an asset into Blend v2, through the gate.
 *
 * The gate is applied *here*, not only in the interface. `supply` re-reads consensus from the
 * registry immediately before it builds the transaction and refuses if the answer is anything
 * but `Verified` — so a stale panel, a hand-edited DOM or a second tab cannot carry a pass that
 * was true a minute ago.
 *
 * Be clear about what that is worth. This is still docs/anchor-integration.md §4a: a
 * client-side check. Someone who calls the pool directly is not stopped by anything here, and
 * nothing in this file should be described as if it were. The on-chain version (§4b) is what
 * would bind, and it is a separate spike.
 *
 * Blend v2's entry point was read off the chain rather than from documentation:
 *   submit(from, spender, to, requests: Vec<Request>) -> Positions
 *   Request { address: Address, amount: i128, request_type: u32 }
 * and `RequestType::Supply = 0` in blend-contracts-v2/pool/src/pool/actions.rs.
 */

import { NETWORK } from '../config.js';
import { loadSdk } from './chain.js';

/** Blend's `RequestType`, the two a depositor would ever use. */
export const REQUEST_TYPE = { Supply: 0, SupplyCollateral: 2 };

/** A decimal amount as an integer of the asset's smallest unit. */
export function toStroops(amount, decimals = 7) {
  const text = String(amount).trim();
  if (!/^\d+(\.\d+)?$/.test(text)) throw new Error(`not an amount: ${text.slice(0, 20)}`);
  const [whole, frac = ''] = text.split('.');
  if (frac.length > decimals) {
    throw new Error(`${text} has more than ${decimals} decimal places`);
  }
  const scaled = `${whole}${frac.padEnd(decimals, '0')}`.replace(/^0+(?=\d)/, '');
  const value = BigInt(scaled);
  if (value <= 0n) throw new Error('amount must be greater than zero');
  return value;
}

/** An integer of the smallest unit, back to a readable decimal. */
export function fromStroops(value, decimals = 7) {
  const s = BigInt(value).toString().padStart(decimals + 1, '0');
  const frac = s.slice(-decimals).replace(/0+$/, '');
  return frac ? `${s.slice(0, -decimals)}.${frac}` : s.slice(0, -decimals);
}

/**
 * Supply `amount` of `asset` into a Blend v2 pool, if the gate allows it.
 *
 * `recheck` is the gate itself, passed in rather than imported, so the one thing that decides
 * whether money moves is visible at the call site and replaceable in a test.
 */
export async function supply({ pool, asset, amount, account, signTransaction, recheck }) {
  // The gate, applied to the pool's *current* bytes, right now. Not the panel's memory of them.
  const verdict = await recheck();
  if (!verdict.allow) {
    const error = new Error(`blocked by the verification gate: ${verdict.headline}`);
    error.verdict = verdict;
    error.blocked = true;
    throw error;
  }

  const sdk = await loadSdk();
  const { rpc: rpcNs, Contract, Address, TransactionBuilder, nativeToScVal, BASE_FEE, xdr } = sdk;
  const server = new rpcNs.Server(NETWORK.rpc);

  const stroops = toStroops(amount, asset.decimals);
  const request = nativeToScVal(
    {
      address: new Address(asset.sac),
      amount: stroops,
      request_type: REQUEST_TYPE.Supply,
    },
    {
      // Soroban structs are maps with symbol keys, and the field types are not inferable from
      // JS values — an i128 amount would otherwise go out as a u32 and be rejected.
      type: {
        address: ['symbol', 'address'],
        amount: ['symbol', 'i128'],
        request_type: ['symbol', 'u32'],
      },
    },
  );

  const from = new Address(account).toScVal();
  const call = new Contract(pool.contractId).call(
    'submit',
    from,
    from, // spender: the depositor pays
    from, // to: and holds the resulting position
    xdr.ScVal.scvVec([request]),
  );

  const source = await server.getAccount(account);
  const built = new TransactionBuilder(source, {
    fee: BASE_FEE,
    networkPassphrase: NETWORK.passphrase,
  })
    .addOperation(call)
    .setTimeout(180)
    .build();

  // Simulation is what discovers the authorization entries and the resource fee; a transaction
  // submitted without it is rejected before it reaches the contract.
  const prepared = await server.prepareTransaction(built);
  const signedXdr = await signTransaction(prepared.toXDR(), {
    networkPassphrase: NETWORK.passphrase,
  });

  const sent = await server.sendTransaction(
    TransactionBuilder.fromXDR(signedXdr, NETWORK.passphrase),
  );
  if (sent.status === 'ERROR') {
    throw new Error(`the pool rejected the transaction: ${JSON.stringify(sent.errorResult ?? {})}`);
  }

  const settled = await waitForTransaction(server, sent.hash);
  return { hash: sent.hash, ...settled, verdict };
}

/** Poll until the network has a verdict on `hash`, or we give up waiting. */
async function waitForTransaction(server, hash, attempts = 30, everyMs = 1000) {
  for (let i = 0; i < attempts; i++) {
    const result = await server.getTransaction(hash);
    if (result.status !== 'NOT_FOUND') {
      return { status: result.status, succeeded: result.status === 'SUCCESS' };
    }
    await new Promise((r) => setTimeout(r, everyMs));
  }
  // Not a failure: the transaction may well land after this. Say which it is.
  return { status: 'PENDING', succeeded: null };
}
