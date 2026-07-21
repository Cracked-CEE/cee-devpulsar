# Anonymous tally — cross-implementation specification & differential harness

**Status:** living document · **Contract spec version:** `tansu 2.0.2` (see
[`contracts/tansu/Cargo.toml`](../../../contracts/tansu/Cargo.toml)) ·
**Owners:** DAO / anonymous-voting

The anonymous tally-and-verify computation is implemented **twice**, in two
languages that must agree:

| Side            | File                                                                                                                  | Role                                                                                                                         |
| --------------- | --------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| Contract (Rust) | [`contract_dao.rs`](../../../contracts/tansu/src/contract_dao.rs)                                                     | `build_commitments_from_votes`, `proof`, `execute` — the on-chain source of truth                                            |
| Client (TS)     | [`anonymousVoting.ts`](../../src/utils/anonymousVoting.ts) → [`anonymousTally.ts`](../../src/utils/anonymousTally.ts) | `computeAnonymousVotingData` accumulation — decrypts votes and reconstructs the tallies/seeds submitted to `proof`/`execute` |

Nothing previously guaranteed they stay in agreement across changes to either
side. This document pins the formula precisely and records the discrepancies the
differential harness surfaced; the harness itself lives beside this file
([TS](./tallyDifferential.test.ts)) and in
[`test_tally_differential.rs`](../../../contracts/tansu/src/tests/test_tally_differential.rs).

---

## 1. The formula, side by side

An anonymous ballot commits, per outcome `i ∈ {approve=0, reject=1, abstain=2}`,
to `C_i = g·voteᵢ + h·seedᵢ` on BLS12-381 G1, where `g`, `h` are the
project-configured generator points and `voteᵢ`, `seedᵢ` are the voter's
**unweighted** decrypted values. Weight is applied only at tally time.

For a proposal with voters `v` each of weight `wᵥ`:

```
tallyᵢ = Σᵥ  wᵥ · voteᵥ,ᵢ
seedᵢ  = Σᵥ  wᵥ · seedᵥ,ᵢ
```

### Contract (`proof`, `contract_dao.rs`)

`proof()` recomputes the aggregate commitment straight from the stored votes and
compares it to the commitment implied by the submitted scalars:

```rust
// aggregate of stored per-voter commitments, scaled by weight
tally_commitmentᵢ = Σᵥ  wᵥ · C_v,ᵢ                       // g1_mul + g1_add
// commitment implied by the submitted (tally, seed) scalars
commitment_checkᵢ = g·tallyᵢ + h·seedᵢ                    // commitment_checks_from_tallies_and_seeds
// proof passes iff, for every i:  tally_commitmentᵢ == commitment_checkᵢ
```

By linearity `Σᵥ wᵥ·(g·voteᵢ + h·seedᵢ) = g·(Σ wᵥ·voteᵢ) + h·(Σ wᵥ·seedᵢ)`, so
the proof passes **iff** the submitted `tallyᵢ`/`seedᵢ` equal `Σ wᵥ·voteᵢ` /
`Σ wᵥ·seedᵢ` **modulo the BLS12-381 scalar field order `r ≈ 2²⁵⁵`**. Scalars are
typed `Vec<u128>` on the contract boundary, and every `u128 < r`, so within
`u128` the reduction never bites — agreement is exact integer agreement.

Weight-application order is irrelevant: integer sums are associative/commutative,
and `weighted_commitment` uses the same linear identity
(`w·(v·G+r·H) == (w·v)·G + (w·r)·H`, proven in
`weighted_commitments_roundtrip_bounds_check`).

### Client (`anonymousTally.ts`, used by `computeAnonymousVotingData`)

```ts
for (const voter of voters)
  for (let i = 0; i < 3; i++) {
    tallies[i] += BigInt(voter.voteValues[i]) * BigInt(voter.weight);
    seeds[i] += BigInt(voter.seedValues[i]) * BigInt(voter.weight);
  }
```

Accumulation is **arbitrary-precision `BigInt`** — deliberately, to avoid the
`u32`/`u53` overflow a `number` tally would hit. This is the intentional
"client uses BigInt where the contract uses u128" choice referenced in the issue.
Within the valid range (below) `BigInt` and `u128` produce the identical integer,
so the choice is safe.

## 2. What "equivalent" means (the valid range)

The two implementations agree **bit-for-bit** when, for every outcome `i`:

1. every decoded `voteᵥ,ᵢ` and `seedᵥ,ᵢ` is a non-negative integer
   `≤ Number.MAX_SAFE_INTEGER (2⁵³−1)` — see **D1**; and
2. `tallyᵢ = Σ wᵥ·voteᵢ` and `seedᵢ = Σ wᵥ·seedᵢ` each fit in `u128` — see **D3**.

The live system stays inside this range by construction:

- `weight` is `u32` and bounded by `get_max_weight` (max badge = `Developer` =
  10 000 000);
- seeds are generated as **32-bit** values
  (`crypto.getRandomValues(new Uint32Array(3))` in
  [`ContractService.ts`](../../src/service/ContractService.ts)); and
- `MAX_VOTES_PER_PROPOSAL = 40`.

So the worst-case weighted seed sum is `40 · 2³² · 2³² ≈ 2⁶⁹ ≪ u128`, and every
seed `< 2³² ≪ 2⁵³`. Within these bounds the harness asserts exact equality.

## 3. Discrepancies found

Per the issue's suggested approach, writing the spec first surfaced three
divergences. None misbehaves inside the live range today, but each is a latent
cross-implementation coupling that a future change to _either_ side could break —
exactly the "confusing user-facing proof failure" the issue warns about. Each is
covered by an explicit, self-documenting assertion in the TS harness.

### D1 — `parseInt` precision loss (client), latent bug

`computeAnonymousVotingData` decodes decrypted values with
`parseInt(plaintext.split(":").pop()!)` (now `decodeAnonymousValue`), which
returns a float64. It is exact only for values `≤ 2⁵³−1`; the contract accepts
seeds up to `u128`. A seed in `[2⁵³, 2¹²⁸)` — valid on-chain — would be silently
rounded client-side, producing a wrong `seedᵢ` and a **failed proof with no
clear cause**.

- **Why it is masked today:** `ContractService.ts` only ever emits 32-bit seeds.
- **Status:** _justified but fragile._ The client's correctness silently depends
  on an invariant enforced only in the vote-casting path. The harness pins the
  `2⁵³` boundary (`decodeAnonymousValue` diverges above it) so any widening of
  the seed range trips a red test.
- **Recommended fix (follow-up):** parse via `BigInt` instead of `parseInt` to
  remove the hidden coupling; tracked separately to keep this PR test-only.

### D2 — proposer weight fallback (client), latent divergence

For the proposer the client uses `Number(data.weight ?? Badge.Verified)`; for
everyone else `?? Badge.Default`. The contract stores and uses a concrete
`weight` (0 for the auto-abstain proposer) uniformly in `proof`. The `??`
fallbacks only fire when `data.weight` is nullish, which the contract never
serializes, so today client and contract both use the same concrete weight.

- **Status:** _justified._ Documented so that any change making `weight`
  optional/nullish is understood to diverge from the contract (relates to the
  historical `WrongVoter` fix — proposer vs. voter handling).

### D3 — unbounded `BigInt` vs. `u128` boundary, latent

The client accumulates in unbounded `BigInt` and hands the result to a `u128`
contract argument with no clamping or validation. If weighted sums ever exceed
`u128::MAX` (only reachable if `MAX_VOTES_PER_PROPOSAL`, the weight cap, or the
seed range grow substantially), the SDK throws while encoding the `BigInt` into
the `u128` `ScVal` — the proof call _errors_ instead of returning `false`.

- **Status:** _justified_ under current constants (worst case `≈ 2⁶⁹`).
- **Recommended fix (follow-up):** validate `tallyᵢ, seedᵢ < 2¹²⁸` before calling
  `proof`/`execute` and surface a clear error. The harness pins the `u128`
  ceiling.

## 4. Re-validating after a contract upgrade

The differential is tied to a spec version (`specVersion` in
[`scenarios.json`](./scenarios.json), currently `tansu-2.0.2`). After **any**
change to `contract_dao.rs`'s commitment/tally/proof logic or a
`contract_migration.rs` bump:

1. Bump `specVersion` in `scenarios.json` to the new contract version.
2. Re-read §1 against the changed Rust and update the formula if it moved.
3. Run both sides — they must stay green (or a new discrepancy must be added to
   §3, each fixed or explicitly justified):
   - `cd dapp && bunx vitest run tests/differential/` (client accumulation vs. oracle)
   - `cargo test -p tansu --lib tally_differential` (real contract `proof()`)
4. If the on-chain generator derivation changes, re-confirm the Rust side still
   builds commitments via `build_commitments_from_votes` (the harness already
   calls the real contract, so it will fail loudly if it drifts).

Both suites read the **same** `scenarios.json`, so a scenario added for a new
edge case is automatically exercised on both sides.
