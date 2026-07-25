# Mutation testing for `contract_dao.rs`

Line/branch coverage proves a test *executed* a piece of code. It doesn't
prove the test would notice if that code were subtly wrong — an off-by-one
in tally accumulation, a flipped comparison in vote-weight validation, an
`&&` that should have been `||`. [cargo-mutants](https://mutants.rs/)
measures that directly: it rewrites the source in small, mechanical ways
(a "mutant" — e.g. `>` becomes `>=`, `+` becomes `-`) and re-runs the test
suite. If every test still passes, the mutant *survived*: that fault would
ship undetected.

This is scoped initially to `contract_dao.rs`: it's the largest module in
the `tansu` contract (~1,400 lines) and carries the highest-risk logic —
proposal lifecycle, vote-weight validation, tally accumulation, and
anonymous-vote commitment verification. Whole-crate mutation testing is not
yet wired into CI because of runtime cost (see [CI integration](#ci-integration)
below); the scope is controlled by `[.cargo/mutants.toml](../../.cargo/mutants.toml)`
and can be widened later.

## Running it locally

```sh
cargo install cargo-mutants --locked
# from the repository root — .cargo/mutants.toml scopes this to contract_dao.rs
cargo mutants
```

A full run takes about 7 minutes on a 10-core machine (145 mutants, ~1-2s
build + ~5-9s test per mutant, `-j 4` in CI). Results land in `mutants.out/`:

- `mutants.out/missed.txt` — mutants no test caught (the actionable list)
- `mutants.out/caught.txt` — mutants a test detected
- `mutants.out/unviable.txt` — mutants that didn't compile (no signal either way)
- `mutants.out/outcomes.json` — full machine-readable report (consumed by
  `.github/workflows/parse_mutants.py` in CI)

To iterate on a single suspicious line: `cargo mutants -F <line-or-name-regex>`.

## Methodology: what counts as the "mutation score"

```
mutation score = caught / (caught + missed)
```

- **Unviable** mutants (didn't compile) are excluded — they carry no
  test-coverage signal, they're just malformed code.
- **Timeout** mutants (test run killed for taking too long — none occurred
  in this baseline) are reported separately, since "the suite hung" is a
  different failure mode from "the suite silently accepted the bug".

## Baseline (before this change)

Run against the pre-existing test suite (`test_anonym_votes.rs`,
`test_dao.rs`, and friends), 145 mutants generated for `contract_dao.rs`:

| Outcome | Count |
|---|---|
| Total mutants | 145 |
| Caught | 93 |
| **Missed** | **28** |
| Timeout | 0 |
| Unviable | 24 |
| **Mutation score** | **76.9%** (93 / 121) |

The 28 missed mutants clustered into eight distinct, meaningful gaps —
listed here with the mutant(s) each represents and the test added to close
it (all in `src/tests/test_dao.rs` unless noted):

1. **The core supermajority decision, `tallies_to_result`, had no boundary
   tests at all** (7 mutants: `contract_dao.rs:1385:22`, `:1385:38`,
   `:1387:28` ×3, `:1387:45` ×2 — `>`→`>=`/`==`/`<`, `+`→`-`/`*`). Every
   existing test used vote weights so lopsided that flipping the
   comparison or swapping `+` for `-`/`*` never changed the outcome. This
   is exactly the "flipped comparison in vote-weight validation" class of
   bug mutation testing exists to catch, and it's the highest-value finding
   here. Fixed by `tallies_to_result_boundary_conditions`, which calls
   `contract_dao::public_execute` directly with exact-tie and
   minimum-margin tallies on both the approve and reject sides.

2. **Pagination arithmetic in `remove_vote`, `revoke_proposal`, and
   `get_proposal`** (9 mutants across `:381:32`, `:382:34`, `:492:32`,
   `:493:34`, `:1019:32`, `:1020:34` — `/`↔`%`↔`*` on
   `page = proposal_id / MAX_PROPOSALS_PER_PAGE` /
   `sub_id = proposal_id % MAX_PROPOSALS_PER_PAGE`). Every test only ever
   addressed `proposal_id 0`, where `0/9`, `0%9`, and `0*9` all happen to
   equal `0`. Fixed by `proposal_addressing_spans_multiple_pages`, which
   creates 11 proposals (spilling onto page 1, since
   `MAX_PROPOSALS_PER_PAGE` is 9) and exercises all three functions there.

3. **`remove_vote`'s voter-list removal used the wrong voter** (1 mutant:
   `:405:57`, `==`→`!=` in `voters.iter().position(|v| v == voter)`). With
   only one extra voter, "find the voter equal to the target" and "find
   the first voter *not* equal" coincidentally remove the same entry.
   Fixed by `remove_vote_removes_exact_voter_from_voters_list` (3 voters,
   remove the last one, assert the *other two* remain).

4. **`vote()`'s anonymous commitment length check only tested with both
   fields broken at once** (1 mutant: `:608:55`, `||`→`&&` on
   `encrypted_votes.len() != 3 || encrypted_seeds.len() != 3`). Fixed by
   `vote_anonymous_commitment_and_encrypted_field_lengths_are_checked_independently`,
   which breaks exactly one field at a time.

5. **`execute()`'s tallies/seeds length check had the same gap** (1
   mutant: `:828:40`, `||`→`&&`). Fixed by
   `execute_tallies_and_seeds_length_checks_are_independent`.

6. **The post-voting timelock boundary was never tested at the boundary**
   (2 mutants: `:754:27` `<`→`<=`, `:754:63` `+`→`-` on
   `curr_timestamp < voting_ends_at + TIMELOCK_DELAY`). Every test executed
   comfortably past the deadline. The `<=` mutant would incorrectly block
   execution at the exact allowed instant; the `+`→`-` mutant would allow
   execution roughly two `TIMELOCK_DELAY`s too early. Fixed by
   `execute_respects_exact_timelock_boundary`.

7. **Outcome-contract execution was only tested for the `Approved` path**
   (2 mutants: `:863:17`, `:864:17` — deleting the `Rejected` / `Cancelled`
   match arms entirely, which fall through to a panic). Fixed by
   `outcomes_execution_rejected` and `outcomes_execution_cancelled` (added
   an `execute_cancel` no-op to the test's `TestOutcomeContract` to mirror
   the existing `execute_approve` / `execute_reject`).

8. **Two collateral/timing constants had no test asserting their exact
   value** (2 mutants: `:13:33` `VOTE_COLLATERAL = 2 * 10_000_000`, `:17:35`
   `MIN_VOTING_PERIOD = 24 * 3600`, both `*`→`+`). Existing tests checked
   *some* collateral moved or *some* voting period was accepted, not the
   exact amount/boundary. Fixed by tightening `proposal_flow`'s balance
   assertion to `assert_eq!` on the exact delta, and by
   `create_proposal_voting_period_boundary` (one second short of the true
   minimum is rejected; exactly the minimum is accepted).

## After this change

Re-running the same 145 mutants against the strengthened suite:

| Outcome | Count |
|---|---|
| Total mutants | 145 |
| Caught | 121 |
| Missed | 0 |
| Timeout | 0 |
| Unviable | 24 |
| **Mutation score** | **100%** (121 / 121) |

All 28 previously-missed mutants are now caught; none were judged
equivalent/untestable, so there was nothing to write a justification for
instead. New (or measurably strengthened) tests live in
`src/tests/test_dao.rs`, in a block marked
`// --- Mutation-testing-driven regression tests ---` with a comment on
each test naming the exact mutant it targets.

100% on the current mutant set is a snapshot, not a guarantee: new code
added to `contract_dao.rs` will generate new mutants that need their own
tests, and cargo-mutants' mutation operators themselves evolve over time.
Re-run `cargo mutants` after non-trivial changes to this file.

## CI integration

`.github/workflows/mutation-testing.yml` runs `cargo mutants` (scoped to
`contract_dao.rs` via `.cargo/mutants.toml`) on every PR/push touching the
contract's DAO logic, and publishes:

- A markdown summary (mutation score, missed/timeout mutant list) in the
  job's GitHub Actions summary, generated by
  `.github/workflows/parse_mutants.py`.
- The full `mutants.out/` report as a build artifact (90-day retention).

**The job is intentionally informational-only for now** (`continue-on-error:
true`): mutation testing is new to this repo, a single run costs several
CPU-minutes (meaningfully more than the regular `cargo test` job, since it
rebuilds and re-tests once per mutant), and there's no team-agreed score
floor to gate on yet. The score is still fully CI-visible; once it's been
observed to be stable across a handful of PRs, flip `continue-on-error` to
`false` (and consider adding a minimum-score check to
`parse_mutants.py`, mirroring the regression-budget pattern in
`.github/workflows/parse_costs.py`) to make it a hard gate.

## Widening scope beyond `contract_dao.rs`

To mutation-test another file, either pass `--file` on the command line
(overrides the config) or edit `examine_globs` in `.cargo/mutants.toml`.
Whole-crate runs (drop `examine_globs` entirely, or pass `--workspace`) are
straightforward locally but will need a longer CI timeout and probably
higher `-j` parallelism or sharding (`--shard i/n`) to stay fast enough to
run on every PR.
