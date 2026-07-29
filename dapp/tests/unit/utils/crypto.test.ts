import { describe, it, expect, beforeAll } from "vitest";
import {
  generateRSAKeyPair,
  generateVoterSigningKeyPair,
  encryptWithPublicKey,
  decryptWithPrivateKey,
  encryptAndSignVote,
  encryptSignAndPackVote,
  packSignedCiphertext,
  unpackSignedCiphertext,
  isSignedCiphertext,
  verifyVoteCiphertextSignature,
  verifyAndDecryptVote,
  VoteCiphertextAuthError,
} from "../../../src/utils/crypto";

// Generate ONE key pair before the suite and share across tests to avoid
// repeated 2048-bit RSA key generation (typically 1–5s per call).
let sharedPublicKey = "";
let sharedPrivateKey = "";
let otherPublicKey = "";
let otherPrivateKey = "";
let voterSigningKey: CryptoKey;
let voterVerifyPubB64 = "";
let otherSigningKey: CryptoKey;

const VOTER_A = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
const VOTER_B = "GBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";

beforeAll(async () => {
  const [pair1, pair2, signing, otherSigning] = await Promise.all([
    generateRSAKeyPair(),
    generateRSAKeyPair(),
    generateVoterSigningKeyPair(),
    generateVoterSigningKeyPair(),
  ]);
  sharedPublicKey = pair1.publicKey;
  sharedPrivateKey = pair1.privateKey;
  otherPublicKey = pair2.publicKey;
  otherPrivateKey = pair2.privateKey;
  voterSigningKey = signing.privateKey;
  voterVerifyPubB64 = signing.publicKey;
  otherSigningKey = otherSigning.privateKey;
}, 30_000);

describe("RSA key generation", () => {
  it("generates a key pair with base-64-encoded keys", () => {
    expect(sharedPublicKey.length).toBeGreaterThan(100);
    expect(sharedPrivateKey.length).toBeGreaterThan(100);
  });

  it("generates different key pairs each time", () => {
    expect(sharedPublicKey).not.toBe(otherPublicKey);
    expect(sharedPrivateKey).not.toBe(otherPrivateKey);
  });
});

describe("RSA encrypt/decrypt round-trip", () => {
  it("encrypts and decrypts a short message", async () => {
    const ciphertext = await encryptWithPublicKey(
      "hello world",
      sharedPublicKey,
    );
    expect(ciphertext).toBeTruthy();
    const plaintext = await decryptWithPrivateKey(ciphertext, sharedPrivateKey);
    expect(plaintext).toBe("hello world");
  });

  it("handles messages with special characters", async () => {
    const message = "a:1:b:2:c:3";
    const ciphertext = await encryptWithPublicKey(message, sharedPublicKey);
    const plaintext = await decryptWithPrivateKey(ciphertext, sharedPrivateKey);
    expect(plaintext).toBe(message);
  });

  it("handles numeric strings", async () => {
    const message = "12345";
    const ciphertext = await encryptWithPublicKey(message, sharedPublicKey);
    const plaintext = await decryptWithPrivateKey(ciphertext, sharedPrivateKey);
    expect(plaintext).toBe(message);
  });

  it("handles empty string", async () => {
    const ciphertext = await encryptWithPublicKey("", sharedPublicKey);
    const plaintext = await decryptWithPrivateKey(ciphertext, sharedPrivateKey);
    expect(plaintext).toBe("");
  });

  it("fails to decrypt with a different private key", async () => {
    const ciphertext = await encryptWithPublicKey("secret", sharedPublicKey);
    await expect(
      decryptWithPrivateKey(ciphertext, otherPrivateKey),
    ).rejects.toThrow();
  });
});

describe("key sanitization", () => {
  it("handles base64 keys with whitespace", async () => {
    const dirtyPublicKey = `  ${sharedPublicKey.slice(0, 20)} \n ${sharedPublicKey.slice(20)} `;
    const dirtyPrivateKey = `\t${sharedPrivateKey.slice(0, 30)}\n${sharedPrivateKey.slice(30)} `;

    const ciphertext = await encryptWithPublicKey("test", dirtyPublicKey);
    const plaintext = await decryptWithPrivateKey(ciphertext, dirtyPrivateKey);
    expect(plaintext).toBe("test");
  });
});

describe("encryptAndSignVote / ciphertext authenticity", () => {
  it("encrypts, signs, and round-trips via verifyAndDecryptVote", async () => {
    const packed = await encryptSignAndPackVote(
      `${VOTER_A}:proj:1:1`,
      sharedPublicKey,
      VOTER_A,
      voterSigningKey,
      voterVerifyPubB64,
    );
    expect(isSignedCiphertext(packed)).toBe(true);
    const plaintext = await verifyAndDecryptVote(
      packed,
      sharedPrivateKey,
      VOTER_A,
    );
    expect(plaintext).toBe(`${VOTER_A}:proj:1:1`);
  });

  it("returns separate ciphertext and signature from encryptAndSignVote", async () => {
    const { ciphertext, signature } = await encryptAndSignVote(
      "vote:1",
      sharedPublicKey,
      voterSigningKey,
      VOTER_A,
    );
    expect(ciphertext.length).toBeGreaterThan(20);
    expect(signature.length).toBeGreaterThan(20);
    await verifyVoteCiphertextSignature(
      ciphertext,
      signature,
      voterVerifyPubB64,
      VOTER_A,
    );
    expect(await decryptWithPrivateKey(ciphertext, sharedPrivateKey)).toBe(
      "vote:1",
    );
  });

  it("keeps legacy raw ciphertexts decryptable (backward compatible)", async () => {
    const legacy = await encryptWithPublicKey("legacy:0", sharedPublicKey);
    expect(isSignedCiphertext(legacy)).toBe(false);
    expect(await decryptWithPrivateKey(legacy, sharedPrivateKey)).toBe(
      "legacy:0",
    );
    expect(
      await verifyAndDecryptVote(legacy, sharedPrivateKey, VOTER_A),
    ).toBe("legacy:0");
  });

  it("rejects decryptWithPrivateKey on signed envelopes with a clear error", async () => {
    const packed = await encryptSignAndPackVote(
      "x",
      sharedPublicKey,
      VOTER_A,
      voterSigningKey,
      voterVerifyPubB64,
    );
    await expect(
      decryptWithPrivateKey(packed, sharedPrivateKey),
    ).rejects.toBeInstanceOf(VoteCiphertextAuthError);
    await expect(decryptWithPrivateKey(packed, sharedPrivateKey)).rejects.toThrow(
      /verifyAndDecryptVote/,
    );
  });

  it("fails signature check with actionable error when ciphertext is tampered", async () => {
    const { ciphertext, signature } = await encryptAndSignVote(
      "ok",
      sharedPublicKey,
      voterSigningKey,
      VOTER_A,
    );
    const tampered = ciphertext.slice(0, -4) + (ciphertext.endsWith("AAAA") ? "BBBB" : "AAAA");
    await expect(
      verifyVoteCiphertextSignature(
        tampered,
        signature,
        voterVerifyPubB64,
        VOTER_A,
      ),
    ).rejects.toBeInstanceOf(VoteCiphertextAuthError);
    await expect(
      verifyVoteCiphertextSignature(
        tampered,
        signature,
        voterVerifyPubB64,
        VOTER_A,
      ),
    ).rejects.toThrow(/tampered|corrupted|verification failed/i);
  });

  it("fails when signature was produced by a different signing key", async () => {
    const { ciphertext } = await encryptAndSignVote(
      "ok",
      sharedPublicKey,
      voterSigningKey,
      VOTER_A,
    );
    const { signature: wrongSig } = await encryptAndSignVote(
      "ok",
      sharedPublicKey,
      otherSigningKey,
      VOTER_A,
    );
    // wrongSig is over a different ciphertext instance; pack with matching
    // ciphertext but wrong sig bytes from another key over same address domain.
    const packed = packSignedCiphertext(
      VOTER_A,
      voterVerifyPubB64,
      wrongSig,
      ciphertext,
    );
    await expect(
      verifyAndDecryptVote(packed, sharedPrivateKey, VOTER_A),
    ).rejects.toBeInstanceOf(VoteCiphertextAuthError);
  });

  it("fails when envelope voter address does not match ballot address", async () => {
    const packed = await encryptSignAndPackVote(
      "ok",
      sharedPublicKey,
      VOTER_A,
      voterSigningKey,
      voterVerifyPubB64,
    );
    await expect(
      verifyAndDecryptVote(packed, sharedPrivateKey, VOTER_B),
    ).rejects.toThrow(/voter mismatch/i);
  });

  it("unpacks a well-formed envelope", () => {
    const packed = packSignedCiphertext(VOTER_A, "pub", "sig", "cipher");
    expect(unpackSignedCiphertext(packed)).toEqual({
      voterAddress: VOTER_A,
      verifyPublicKeyB64: "pub",
      signature: "sig",
      ciphertext: "cipher",
    });
  });
});
