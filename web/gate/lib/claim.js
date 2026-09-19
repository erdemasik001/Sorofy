/**
 * The claim digest, computed in the browser.
 *
 * This is the third implementation of one byte layout. The registry has it
 * (`contracts/registry/src/claim.rs`), the API has it (`crates/api/src/claim.rs`), and now the
 * gate does. They only agree because all three are pinned to the same known-answer vectors,
 * which were produced independently in Python — see test/claim.test.js.
 *
 *   input_digest = sha256( "sorofy-claim-v1"
 *                          ‖ source_sha256        32 bytes
 *                          ‖ bldimg_digest        32 bytes  (the hex after `@sha256:`)
 *                          ‖ u32_be(len(bldopt))
 *                          ‖ for each bldopt, in submission order: u32_be(len) ‖ bytes )
 *
 * Why the gate computes it at all: the registry files every attestation under
 * `(wasm_hash, input_digest)` and offers no "is any claim for this hash verified" query. The
 * gate has the wasm hash from the chain; to ask about it, it has to be able to name the claim.
 */

const DOMAIN = new TextEncoder().encode('sorofy-claim-v1');

/** Lowercase hex → bytes. Throws on anything that is not exactly `expectedLen` bytes of hex. */
export function fromHex(hex, expectedLen = null) {
  const s = String(hex).trim().toLowerCase();
  if (!/^[0-9a-f]*$/.test(s) || s.length % 2 !== 0) {
    throw new Error(`not hex: ${truncate(s)}`);
  }
  const out = new Uint8Array(s.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(s.substr(i * 2, 2), 16);
  if (expectedLen !== null && out.length !== expectedLen) {
    throw new Error(`expected ${expectedLen} bytes, got ${out.length}`);
  }
  return out;
}

/** Bytes → lowercase hex. */
export function toHex(bytes) {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

function truncate(s) {
  return s.length > 24 ? `${s.slice(0, 24)}…` : s;
}

function u32be(n) {
  const b = new Uint8Array(4);
  new DataView(b.buffer).setUint32(0, n, false);
  return b;
}

function concat(chunks) {
  const total = chunks.reduce((n, c) => n + c.length, 0);
  const out = new Uint8Array(total);
  let at = 0;
  for (const c of chunks) {
    out.set(c, at);
    at += c.length;
  }
  return out;
}

/**
 * The 32 bytes the registry files a claim under.
 *
 * `sourceSha256` and `bldimgDigest` are 64-char hex; `bldopt` is the flag list **in submission
 * order**, because the order is part of the question and changes the digest.
 */
export async function inputDigest({ sourceSha256, bldimgDigest, bldopt }) {
  const opts = bldopt ?? [];
  const encoder = new TextEncoder();
  const parts = [DOMAIN, fromHex(sourceSha256, 32), fromHex(bldimgDigest, 32), u32be(opts.length)];
  for (const opt of opts) {
    const bytes = encoder.encode(opt);
    // Length-prefixed, or ["ab","c"] and ["a","bc"] would hash identical bytes.
    parts.push(u32be(bytes.length), bytes);
  }
  const hash = await crypto.subtle.digest('SHA-256', concat(parts));
  return new Uint8Array(hash);
}

/** [`inputDigest`] as lowercase hex. */
export async function inputDigestHex(descriptor) {
  return toHex(await inputDigest(descriptor));
}

/**
 * The hex after `@sha256:` in a `repo@sha256:…` reference, or the string if it is already a
 * bare digest.
 *
 * A reference with no digest is refused rather than guessed at: that is the unpinned-image
 * case, and inventing a digest would name a claim no verifier could ever have attested.
 */
export function imageDigestHex(bldimg) {
  const s = String(bldimg).trim();
  const at = s.indexOf('@sha256:');
  if (at >= 0) return s.slice(at + '@sha256:'.length).toLowerCase();
  if (/^[0-9a-fA-F]{64}$/.test(s)) return s.toLowerCase();
  throw new Error(`build image is not pinned by digest: ${truncate(s)}`);
}
