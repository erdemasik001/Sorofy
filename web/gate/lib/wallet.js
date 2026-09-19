/**
 * Stellar Wallets Kit — one wallet connection, three jobs.
 *
 * The same connection signs the SEP-10 challenge, the trustline and the deposit. That is why
 * docs/anchor-integration.md calls the Kit load-bearing rather than decorative: remove it and
 * there is no way to authenticate to the anchor *or* to authorize the deposit.
 *
 * Nothing above this module knows which wallet was chosen. It gets an address and a
 * `signTransaction(xdr, opts) -> signedXdr`, which is the whole surface the rest of the app
 * needs and the whole surface worth depending on.
 */

import { VENDOR, NETWORK } from '../config.js';

let kitPromise = null;

async function loadKit() {
  if (!kitPromise) {
    kitPromise = import(/* @vite-ignore */ VENDOR.walletsKit).catch((e) => {
      kitPromise = null;
      throw new Error(`could not load Stellar Wallets Kit from ${VENDOR.walletsKit}: ${e.message}`);
    });
  }
  return kitPromise;
}

let kit = null;

/**
 * Open the wallet picker and connect.
 *
 * Returns `{ address, signTransaction }`. Throws if the user dismisses the picker, which is a
 * normal outcome and not an error worth decorating.
 */
export async function connect() {
  const mod = await loadKit();
  const { StellarWalletsKit, WalletNetwork, allowAllModules } = mod;

  if (!kit) {
    kit = new StellarWalletsKit({
      network: WalletNetwork.TESTNET,
      modules: allowAllModules(),
    });
  }

  const selectedId = await new Promise((resolve, reject) => {
    kit
      .openModal({
        onWalletSelected: (option) => resolve(option.id),
        onClosed: () => reject(new Error('no wallet selected')),
      })
      .catch(reject);
  });
  kit.setWallet(selectedId);

  const { address } = await kit.getAddress();
  if (!address) throw new Error('the wallet returned no address');

  return { address, walletId: selectedId, signTransaction: signWith(address) };
}

/**
 * A signer bound to one address.
 *
 * The Kit has returned the signed envelope under different names across versions, so the result
 * is unwrapped by shape rather than by a single property — a silent `undefined` here would be
 * submitted as an empty transaction.
 */
function signWith(address) {
  return async (xdr, opts = {}) => {
    const result = await kit.signTransaction(xdr, {
      address,
      networkPassphrase: opts.networkPassphrase ?? NETWORK.passphrase,
    });
    const signed = typeof result === 'string' ? result : (result?.signedTxXdr ?? result?.signedXDR);
    if (typeof signed !== 'string' || signed.length === 0) {
      throw new Error('the wallet returned no signed transaction');
    }
    return signed;
  };
}

/** Forget the connection. The page holds no key, so this is all there is to disconnect. */
export async function disconnect() {
  try {
    await kit?.disconnect?.();
  } catch {
    // A wallet that does not implement disconnect is not a failure worth surfacing.
  }
}
