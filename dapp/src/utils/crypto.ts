// crypto utility for anonymous voting (recreated)
// Provides RSA-OAEP key-pair generation, encrypt/decrypt, and ECDSA binding
// of vote ciphertexts to the submitting voter (see docs/anonymous-vote-ciphertext-threat-model.md).

/** Domain-separated payload prefix for vote ciphertext signatures. */
export const VOTE_CIPHERTEXT_SIG_DOMAIN = "TANSU_ANON_VOTE_V1";

/** Packed signed-ciphertext envelope version prefix. */
export const SIGNED_CIPHERTEXT_PREFIX = "tansu-sv1";

export class VoteCiphertextAuthError extends Error {
  constructor(message: string, options?: { cause?: unknown }) {
    super(message, options);
    this.name = "VoteCiphertextAuthError";
  }
}

// Generate a 2048-bit RSA-OAEP key pair and return base-64 strings.
export async function generateRSAKeyPair(): Promise<{
  publicKey: string;
  privateKey: string;
}> {
  const keyPair = await crypto.subtle.generateKey(
    {
      name: "RSA-OAEP",
      modulusLength: 2048,
      publicExponent: new Uint8Array([0x01, 0x00, 0x01]),
      hash: "SHA-256",
    },
    true,
    ["encrypt", "decrypt"],
  );

  const [pubBuf, privBuf] = await Promise.all([
    crypto.subtle.exportKey("spki", keyPair.publicKey),
    crypto.subtle.exportKey("pkcs8", keyPair.privateKey),
  ]);

  return { publicKey: bufToB64(pubBuf), privateKey: bufToB64(privBuf) };
}

/**
 * Generate an ECDSA P-256 key pair used to bind ciphertexts to a voter.
 *
 * "Valid" on the client means a signature under this key verifies over
 * `TANSU_ANON_VOTE_V1|<voterAddress>|<ciphertext>`. Contract-side verification
 * with the voter's Stellar Ed25519 account key is a follow-up (see threat model).
 */
export async function generateVoterSigningKeyPair(): Promise<{
  publicKey: string;
  privateKey: CryptoKey;
  publicCryptoKey: CryptoKey;
}> {
  const keyPair = await crypto.subtle.generateKey(
    { name: "ECDSA", namedCurve: "P-256" },
    true,
    ["sign", "verify"],
  );
  const pubBuf = await crypto.subtle.exportKey("spki", keyPair.publicKey);
  return {
    publicKey: bufToB64(pubBuf),
    privateKey: keyPair.privateKey,
    publicCryptoKey: keyPair.publicKey,
  };
}

// ────────────────── Encryption ──────────────────
export async function encryptWithPublicKey(
  plaintext: string,
  publicKeyB64: string,
): Promise<string> {
  const key = await importPublicKey(publicKeyB64);
  const enc = new TextEncoder().encode(plaintext);
  const cipher = await crypto.subtle.encrypt(
    { name: "RSA-OAEP" },
    key,
    enc as BufferSource,
  );
  return bufToB64(cipher);
}

/**
 * Encrypt a vote/seed plaintext and sign the ciphertext with the voter's
 * signing key so tampering is detectable before decrypt/tally.
 *
 * The signature covers `TANSU_ANON_VOTE_V1|<voterAddress>|<ciphertext>` when
 * `voterAddress` is provided (production path). If omitted, only the
 * ciphertext bytes are signed (unit-test convenience matching the issue stub).
 */
export async function encryptAndSignVote(
  plaintext: string,
  votingPublicKeyB64: string,
  voterSigningKey: CryptoKey,
  voterAddress = "",
): Promise<{ ciphertext: string; signature: string }> {
  const ciphertext = await encryptWithPublicKey(plaintext, votingPublicKeyB64);
  const payload = voteSignaturePayload(voterAddress, ciphertext);
  const signatureBuf = await crypto.subtle.sign(
    { name: "ECDSA", hash: "SHA-256" },
    voterSigningKey,
    payload,
  );
  return { ciphertext, signature: bufToB64(signatureBuf) };
}

/**
 * Pack a signed ciphertext into a single string suitable for on-chain
 * `encrypted_votes` / `encrypted_seeds` fields (no contract schema change).
 */
export function packSignedCiphertext(
  voterAddress: string,
  verifyPublicKeyB64: string,
  signature: string,
  ciphertext: string,
): string {
  if (!voterAddress || !verifyPublicKeyB64 || !signature || !ciphertext) {
    throw new VoteCiphertextAuthError(
      "Cannot pack signed ciphertext: missing voter address, verify key, signature, or ciphertext",
    );
  }
  return [
    SIGNED_CIPHERTEXT_PREFIX,
    voterAddress,
    verifyPublicKeyB64,
    signature,
    ciphertext,
  ].join(".");
}

export interface UnpackedSignedCiphertext {
  voterAddress: string;
  verifyPublicKeyB64: string;
  signature: string;
  ciphertext: string;
}

export function isSignedCiphertext(value: string): boolean {
  return value.startsWith(`${SIGNED_CIPHERTEXT_PREFIX}.`);
}

export function unpackSignedCiphertext(packed: string): UnpackedSignedCiphertext {
  const parts = packed.split(".");
  if (parts.length !== 5 || parts[0] !== SIGNED_CIPHERTEXT_PREFIX) {
    throw new VoteCiphertextAuthError(
      "Malformed signed vote ciphertext envelope (expected tansu-sv1.<voter>.<pub>.<sig>.<cipher>)",
    );
  }
  const [, voterAddress, verifyPublicKeyB64, signature, ciphertext] = parts;
  if (!voterAddress || !verifyPublicKeyB64 || !signature || !ciphertext) {
    throw new VoteCiphertextAuthError(
      "Malformed signed vote ciphertext envelope: empty field",
    );
  }
  return { voterAddress, verifyPublicKeyB64, signature, ciphertext };
}

/**
 * Encrypt + sign + pack for storage. Used by the vote-casting path.
 */
export async function encryptSignAndPackVote(
  plaintext: string,
  votingPublicKeyB64: string,
  voterAddress: string,
  voterSigningKey: CryptoKey,
  verifyPublicKeyB64: string,
): Promise<string> {
  const { ciphertext, signature } = await encryptAndSignVote(
    plaintext,
    votingPublicKeyB64,
    voterSigningKey,
    voterAddress,
  );
  return packSignedCiphertext(
    voterAddress,
    verifyPublicKeyB64,
    signature,
    ciphertext,
  );
}

// ────────────────── Decryption ──────────────────
export async function decryptWithPrivateKey(
  cipherB64: string,
  privateKeyB64: string,
): Promise<string> {
  if (isSignedCiphertext(cipherB64)) {
    throw new VoteCiphertextAuthError(
      "Signed vote ciphertext requires verifyAndDecryptVote (signature check before decrypt)",
    );
  }
  const key = await importPrivateKey(privateKeyB64);
  const cipherBuf = b64ToBuf(cipherB64);
  const plainBuf = await crypto.subtle.decrypt(
    { name: "RSA-OAEP" },
    key,
    cipherBuf,
  );
  return new TextDecoder().decode(plainBuf);
}

/**
 * Verify an ECDSA signature over a vote ciphertext.
 * Throws VoteCiphertextAuthError with an actionable message on failure.
 */
export async function verifyVoteCiphertextSignature(
  ciphertext: string,
  signatureB64: string,
  verifyPublicKeyB64: string,
  voterAddress = "",
): Promise<void> {
  let verifyKey: CryptoKey;
  try {
    verifyKey = await importEcdsaVerifyKey(verifyPublicKeyB64);
  } catch (e) {
    throw new VoteCiphertextAuthError(
      "Vote ciphertext signature check failed: could not import voter verify key",
      { cause: e },
    );
  }

  const payload = voteSignaturePayload(voterAddress, ciphertext);
  let ok = false;
  try {
    ok = await crypto.subtle.verify(
      { name: "ECDSA", hash: "SHA-256" },
      verifyKey,
      b64ToBuf(signatureB64),
      payload,
    );
  } catch (e) {
    throw new VoteCiphertextAuthError(
      `Vote ciphertext signature check failed for voter ${voterAddress || "(unknown)"}: invalid signature encoding`,
      { cause: e },
    );
  }

  if (!ok) {
    throw new VoteCiphertextAuthError(
      `Vote ciphertext signature verification failed for voter ${voterAddress || "(unknown)"}. ` +
        "The encrypted ballot may have been tampered with or corrupted in transit/storage. " +
        "Do not fold this ballot into the tally; remove or re-cast the vote after investigating.",
    );
  }
}

/**
 * Authenticate (when signed) then decrypt a vote/seed field.
 *
 * - Legacy raw RSA-OAEP ciphertext: decrypt only (backward compatible).
 * - Signed envelope: require `expectedVoterAddress` match, verify signature,
 *   then decrypt the inner ciphertext with `decryptWithPrivateKey`.
 */
export async function verifyAndDecryptVote(
  packedOrLegacyCipher: string,
  votingPrivateKeyB64: string,
  expectedVoterAddress?: string,
): Promise<string> {
  if (!isSignedCiphertext(packedOrLegacyCipher)) {
    return decryptWithPrivateKey(packedOrLegacyCipher, votingPrivateKeyB64);
  }

  const unpacked = unpackSignedCiphertext(packedOrLegacyCipher);

  if (
    expectedVoterAddress &&
    unpacked.voterAddress !== expectedVoterAddress
  ) {
    throw new VoteCiphertextAuthError(
      `Vote ciphertext voter mismatch: envelope claims ${unpacked.voterAddress} but ballot address is ${expectedVoterAddress}. ` +
        "Possible ciphertext substitution between voters — refusing to decrypt.",
    );
  }

  await verifyVoteCiphertextSignature(
    unpacked.ciphertext,
    unpacked.signature,
    unpacked.verifyPublicKeyB64,
    unpacked.voterAddress,
  );

  return decryptWithPrivateKey(unpacked.ciphertext, votingPrivateKeyB64);
}

// ────────────────── Helpers ──────────────────

function voteSignaturePayload(
  voterAddress: string,
  ciphertext: string,
): Uint8Array {
  return new TextEncoder().encode(
    `${VOTE_CIPHERTEXT_SIG_DOMAIN}|${voterAddress}|${ciphertext}`,
  );
}

function bufToB64(buf: ArrayBuffer): string {
  return btoa(String.fromCharCode(...new Uint8Array(buf)));
}

function b64ToBuf(b64: string): ArrayBuffer {
  // Removes spaces, tabs, newlines, and non-base64-safe characters
  b64 = b64.replace(/[^A-Za-z0-9+/=]/g, "");

  const bin = atob(b64);
  return Uint8Array.from(bin, (c) => c.charCodeAt(0)).buffer;
}

async function importPublicKey(b64: string): Promise<CryptoKey> {
  const clean = b64.replace(/[^A-Za-z0-9+/=]/g, "");
  return crypto.subtle.importKey(
    "spki",
    b64ToBuf(clean),
    { name: "RSA-OAEP", hash: "SHA-256" },
    true,
    ["encrypt"],
  );
}

async function importPrivateKey(b64: string): Promise<CryptoKey> {
  const clean = b64.replace(/[^A-Za-z0-9+/=]/g, "");
  return crypto.subtle.importKey(
    "pkcs8",
    b64ToBuf(clean),
    { name: "RSA-OAEP", hash: "SHA-256" },
    true,
    ["decrypt"],
  );
}

async function importEcdsaVerifyKey(b64: string): Promise<CryptoKey> {
  const clean = b64.replace(/[^A-Za-z0-9+/=]/g, "");
  return crypto.subtle.importKey(
    "spki",
    b64ToBuf(clean),
    { name: "ECDSA", namedCurve: "P-256" },
    true,
    ["verify"],
  );
}
