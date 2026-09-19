/**
 * A reader for the handful of `ScVal` shapes the registry's events use.
 *
 * Not a general XDR implementation and not a replacement for the SDK's. It exists so the event
 * scan — the part that decides *which claims exist* — needs no SDK, runs in Node, and can be
 * pinned by a test to bytes captured from the live network rather than to a library's promise
 * about them.
 *
 * Every value it cannot read throws. A decoder that guessed would turn a decoding bug into a
 * wrong claim, and a wrong claim reads as `NoClaim`, which blocks — so the failure would look
 * like a verdict.
 */

// ScVal discriminants (stellar-xdr), only the ones these events contain.
const U32 = 3;
const SYMBOL = 15;
const BYTES = 13;
const VEC = 16;
const MAP = 17;
const ADDRESS = 18;

/** base64 → bytes, without Buffer. */
export function b64ToBytes(b64) {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function hex(bytes) {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

/** XDR pads every variable-length field out to a multiple of four bytes. */
const padded = (n) => n + ((4 - (n % 4)) % 4);

class Reader {
  constructor(bytes) {
    this.b = bytes;
    this.at = 0;
    this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }

  u32() {
    if (this.at + 4 > this.b.length) throw new Error('XDR: ran off the end');
    const v = this.view.getUint32(this.at, false);
    this.at += 4;
    return v;
  }

  bytes(n) {
    if (this.at + n > this.b.length) throw new Error('XDR: ran off the end');
    const v = this.b.subarray(this.at, this.at + n);
    this.at += padded(n);
    return v;
  }

  value() {
    const type = this.u32();
    switch (type) {
      case U32:
        return this.u32();
      case SYMBOL:
        return new TextDecoder().decode(this.bytes(this.u32()));
      case BYTES:
        return hex(this.bytes(this.u32()));
      case ADDRESS: {
        // SCAddress: 0 = account (then a PublicKey union), 1 = contract (then 32 bytes).
        const kind = this.u32();
        if (kind === 0) this.u32(); // PUBLIC_KEY_TYPE_ED25519
        return { kind: kind === 0 ? 'account' : 'contract', key: hex(this.bytes(32)) };
      }
      case VEC:
      case MAP: {
        // Both are XDR *optional* containers: a presence flag, then the length.
        const present = this.u32();
        if (!present) return type === VEC ? [] : {};
        const len = this.u32();
        if (type === VEC) {
          return Array.from({ length: len }, () => this.value());
        }
        const out = {};
        for (let i = 0; i < len; i++) {
          const k = this.value();
          out[k] = this.value();
        }
        return out;
      }
      default:
        throw new Error(`XDR: unsupported ScVal type ${type}`);
    }
  }
}

/** Decode one base64 `ScVal`. Throws on anything this reader does not cover. */
export function decodeScVal(b64) {
  return new Reader(b64ToBytes(b64)).value();
}
