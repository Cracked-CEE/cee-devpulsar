# Reproducible Build Attestation — Research & Local Reproduction Recipe

## Status

> **Current state**: Greenfield — no reproducibility check exists in any CI workflow.
> `contract-release.yml` uses `stellar-expert/soroban-build-workflow` (community reusable workflow)
> which builds `contracts/tansu` but produces **no attestation** binding source commit → WASM hash.

## Why Reproducible Builds Matter

An external auditor reviewing `contracts/domain_current.wasm` currently has **no proof** that
the deployed WASM was produced deterministically from the published Rust source at a given
commit. This is a supply-chain gap between "source looks correct" and "the bytes on-chain
match that source."

A reproducible-build attestation bridges this gap by producing a **signed statement**:
- Source commit hash
- Build environment (toolchain version, OS, CI runner)
- WASM content hash (SHA256)

A third party can independently rebuild from source and verify the attestation **without
trusting this CI account**, using Sigstore/GitHub attestations.

## Known Rust/Soroban WASM Reproducibility Pitfalls

Research completed on 2026-07-19. Below are the known non-determinism sources in Rust
WASM builds, specifically for Soroban contracts.

### 1. Embedded Timestamps (HIGH IMPACT)

The `crate-git-revision` crate (used by `soroban-env-common`, `soroban-sdk`, `stellar-xdr`)
embeds the **crate version's build timestamp** into the compiled binary.

**Mitigation**: Use `SOURCE_DATE_EPOCH` environment variable to pin the timestamp.
Set `SOURCE_DATE_EPOCH` to the commit timestamp of the current checkout.

```bash
export SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)
```

This is the single most impactful fix and is **required** for deterministic builds.

### 2. Absolute Build Paths (MEDIUM IMPACT)

Rust's `panic = "abort"` and debug info can embed absolute file paths. The existing profile
sets `debug = 0` and `strip = "symbols"` which helps, but build scripts (`build.rs`) may
still embed paths.

**Mitigation**: 
- The workspace `Cargo.toml` already has `debug = 0` and `strip = "symbols"` — good.
- Ensure `RUSTFLAGS` does not include `--remap-path-prefix` unless needed (can mask real issues).
- For maximum determinism, consider building in a Docker container with identical paths.

### 3. HashMap / HashSet Iteration Order (LOW IMPACT)

Rust's `std::collections::HashMap` uses SipHash with a random seed by default, making
iteration order non-deterministic. However, in WASM builds:
- The `wasm32v1-none` target uses a deterministic hash seed (no OS entropy source).
- Soroban SDK v27 uses `hashbrown` which is also deterministic in no-std environments.

**Risk**: Low for Soroban v27. Accepted.

### 4. Link-Time Optimization (LTO) (MEDIUM IMPACT)

LTO behavior can vary between compiler versions. The workspace already pins `lto = true`
and `codegen-units = 1`. This is good but requires **exact same compiler version**.

**Mitigation**: Pin the Rust toolchain version explicitly (see `rust-toolchain.toml`).

### 5. Stellar CLI Version (HIGH IMPACT)

The `stellar contract build` command uses `stellar-cli` which wraps the compiler.
Different `stellar-cli` versions may produce different WASM output.

**Mitigation**: Pin the exact `stellar-cli` version in CI. Match the version used in
`contract-release.yml` (which uses `stellar-expert/soroban-build-workflow`).

### 6. WASM Metadata Sections (MEDIUM IMPACT)

Soroban's build process can add metadata sections (name, producers) to the WASM binary.
These may include producer tool versions and timestamps.

**Mitigation**: Use `wasm-opt` with `--strip-debug` and `--strip-producers` flags, or
use `stellar contract build --optimize` which already applies WASM optimization.

## Local Reproduction Recipe

Follow these steps to manually verify reproducible builds locally.

### Prerequisites

```bash
# Install Rust (stable, same version as CI)
rustup install stable  # or pinned version from rust-toolchain.toml
rustup target add wasm32v1-none

# Install stellar-cli (match CI version)
cargo install --locked stellar-cli --version <VERSION_FROM_CI>

# Install tools for hashing
# (sha256sum is available on Linux/macOS, or use openssl on Windows)
```

### Step-by-Step Reproduction

```bash
# 1. Checkout the exact commit being verified
git checkout <COMMIT_HASH>

# 2. Set SOURCE_DATE_EPOCH from the commit timestamp
export SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)
export RUSTFLAGS="-C debuginfo=0"

# 3. Build contracts/tansu
cd contracts/tansu
cargo build --locked --release --target wasm32v1-none
cd ../..

# 4. Build contracts/scf-membership
cd contracts/scf-membership
cargo build --locked --release --target wasm32v1-none
cd ../..

# 5. Compute WASM hashes
sha256sum target/wasm32v1-none/release/tansu.wasm
sha256sum target/wasm32v1-none/release/scf_membership.wasm

# 6. Compare against known deployed artifacts
# For tansu: compare with contracts/domain_current.wasm
sha256sum contracts/domain_current.wasm

# 7. Verify reproducibility: build a SECOND time
# (clean first to ensure no stale artifacts)
cargo clean
export SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)
cargo build --locked --release --target wasm32v1-none

# 8. Compare hashes from both builds — they MUST match byte-for-byte
sha256sum target/wasm32v1-none/release/tansu.wasm
# ^^ Should produce the SAME hash as step 5
```

### Verifying the Attestation

If CI produced an attestation artifact, verify it with:

```bash
# Using GitHub CLI
gh attestation verify target/wasm32v1-none/release/tansu.wasm \
  --owner <ORG> \
  --repo <REPO>

# Or using cosign directly
cosign verify-blob-attestation \
  --signature attestation.sig \
  --certificate attestation.crt \
  target/wasm32v1-none/release/tansu.wasm
```

## Known Non-Determinism Sources (Accepted/Flagged)

| Source | Impact | Mitigation | Status |
|--------|--------|------------|--------|
| `crate-git-revision` timestamps | HIGH | `SOURCE_DATE_EPOCH` | Mitigated |
| Absolute build paths | MEDIUM | `debug=0`, `strip=symbols` | Mitigated |
| LLVM/LTO version differences | MEDIUM | Pinned Rust toolchain | Mitigated |
| `stellar-cli` version | HIGH | Pinned via `cargo install --locked` | Mitigated |
| HashMap iteration order | LOW | No-std target → deterministic by default | Accepted |
| WASM metadata sections | MEDIUM | `--optimize` strips most metadata | Mitigated |
| System locale / timezone | LOW | Docker build container | Accepted with Docker |

**Accepted Risks**: 
- System locale may affect error messages embedded in panic strings. Since `panic = "abort"`
  is set, this is minimized.
- Docker build image version must match (Ubuntu 22.04 vs 24.04 may have different system
  libraries). CI uses `ubuntu-latest`; local reproduction should use same.

## Future Improvements

- [ ] Docker-based build container for fully hermetic builds
- [ ] Automated weekly reproducibility cross-check with Soroban mainnet artifacts
- [ ] Integration with `sigstore` for keyless signing outside GitHub
- [ ] Multi-platform verification (arm64 CI runners match x86_64 builds exactly)

