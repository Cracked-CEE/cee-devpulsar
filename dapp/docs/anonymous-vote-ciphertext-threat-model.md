# Anonymous vote ciphertext authenticity — threat model (#46)

## Components

| Layer | What it protects | Mechanism |
| --- | --- | --- |
| RSA-OAEP (`encryptWithPublicKey`) | Confidentiality of vote/seed plaintexts until reveal | Project anonymous-voting public key |
| Pedersen commitments (`build_commitments_from_votes`) | Binding of plaintext vote/seed scalars | \(C = g\cdot v + h\cdot r\) on BLS12-381 |
| Aggregate `proof()` / `execute()` | Tallies match Σ weighted commitments | On-chain G1 accumulation check |
| Stellar tx `require_auth` | Ballot submission authenticity on-chain | Voter signs the Soroban transaction |
| **Ciphertext signature (this fix)** | Integrity/authenticity of ciphertext *bytes* before decrypt | ECDSA P-256 over ciphertext, bound to voter address |

## What `proof()` / commitments already catch

1. **Substituting ciphertext with encryption of different plaintexts** while leaving commitments unchanged: reveal decrypts to values whose weighted tallies do not match on-chain commitments → `proof()` / `execute()` reject.
2. **Forging tallies at execute time**: maintainer-supplied tallies/seeds must satisfy the commitment check; garbage tallies cannot pass `execute()`.
3. **Unauthorized on-chain ballot replacement**: changing stored `encrypted_*` / `commitments` requires a new authorized transaction from the voter (or contract admin paths outside this threat model).

## Gap that remains (fixed here)

RSA-OAEP provides **confidentiality only** — no MAC or signature on the ciphertext.

| Tamper | Without ciphertext auth | With ciphertext signature |
| --- | --- | --- |
| Bit-flip / storage or relay corruption of `encrypted_*` strings | Opaque WebCrypto decrypt failure (or confusing OAEP error) with no indication *which* field failed auth | Clear error: signature verification failed for voter `G…` before decrypt is attempted |
| Off-chain copy/mix-up of ciphertexts between voters (bug) | Decrypt may succeed into the wrong ballot’s values; failure surfaces only as aggregate `proof()` false | Address-bound signature fails when envelope voter ≠ `AnonymousVote.address` or sig does not verify |
| Malicious re-encryption under the project pubkey with a new ephemeral signing key | Same as commitment mismatch (caught late by `proof()`) | Client detects bad sig only if verify key is trusted; **contract-side check of the voter’s Stellar Ed25519 key over the ciphertext is a follow-up** (see below) |

Commitments do **not** bind ciphertext bytes to the committed scalars. They only check that *whatever was decrypted and tallied* matches the commitments. That is necessary for execution safety, but it is a late, aggregate signal — it does not give a per-ballot, pre-decrypt authenticity error.

## What “valid” means for verification

- **Client (this PR):** A packed ciphertext `tansu-sv1.<voter>.<ecdsaPub>.<sig>.<cipher>` is valid iff:
  1. `voter` equals the on-chain `AnonymousVote.address`, and
  2. ECDSA P-256 / SHA-256 verifies `sig` over `TANSU_ANON_VOTE_V1|<voter>|<cipher>` under `ecdsaPub`, and
  3. RSA-OAEP decrypt of `cipher` succeeds under the project private key.
- **Legacy:** Raw base64 RSA-OAEP ciphertexts (votes cast before this change) skip signature checks and remain decryptable via `decryptWithPrivateKey` / `verifyAndDecryptVote`.
- **Contract follow-up (out of scope):** `contract_dao.rs` should eventually require an Ed25519 signature from the voter’s Stellar account key over each ciphertext (or a batch digest) at `vote()`, so authenticity does not rely on an embedded ECDSA key. Flagged here so client and contract work stay separable.

## Non-goals (unchanged)

- On-chain proof that encrypted values are one-hot / in-range (see audit notes on anonymous ballot validity).
- Replacing Pedersen commitments or `proof()` algebra.
- AEAD under the project key alone (would not bind voter identity without additional AD binding and key-management changes).
