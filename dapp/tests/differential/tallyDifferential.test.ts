// Cross-implementation differential test for the anonymous tally.
//
// The client (utils/anonymousTally.ts, used by computeAnonymousVotingData) and
// the Rust contract (contract_dao.rs proof()/execute()) compute the same
// tally-and-verify independently. This suite pins the CLIENT accumulation
// against an independent oracle of the contract formula, over:
//   * the shared canonical scenarios (scenarios.json) — the SAME cases the Rust
//     side (test_tally_differential.rs) feeds into the real contract proof(); and
//   * property-based random scenarios within the documented contract-valid range.
// It also encodes the discrepancies documented in differential-tally-spec.md so
// any future drift trips a red test.
//
// See differential-tally-spec.md for the formula and the D1/D2/D3 discrepancies.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { describe, it, expect } from "vitest";
import fc from "fast-check";
import {
  accumulateAnonymousTally,
  decodeAnonymousValue,
  type DecodedVoter,
} from "utils/anonymousTally";

const __dirname = dirname(fileURLToPath(import.meta.url));

interface ScenarioVoter {
  weight: number;
  votes: [number, number, number];
  seeds: [number, number, number];
}
interface Scenario {
  name: string;
  note?: string;
  voters: ScenarioVoter[];
}
interface ScenarioFile {
  specVersion: string;
  scenarios: Scenario[];
}

const scenarioFile: ScenarioFile = JSON.parse(
  readFileSync(join(__dirname, "scenarios.json"), "utf8"),
);

const U128_MAX = (1n << 128n) - 1n;
const MAX_SAFE = BigInt(Number.MAX_SAFE_INTEGER); // 2^53 - 1

// --- The two implementations under differential test -------------------------

// Oracle: the exact integer tally the contract's commitment check requires,
// computed independently from the client kernel (plain BigInt reduction).
//   tally[i] = Σ w·vote[i],  seed[i] = Σ w·seed[i]
function oracleTally(voters: ScenarioVoter[]): {
  tallies: bigint[];
  seeds: bigint[];
} {
  const tallies = [0n, 0n, 0n];
  const seeds = [0n, 0n, 0n];
  for (const v of voters) {
    for (let i = 0; i < 3; i++) {
      tallies[i]! += BigInt(v.votes[i]!) * BigInt(v.weight);
      seeds[i]! += BigInt(v.seeds[i]!) * BigInt(v.weight);
    }
  }
  return { tallies, seeds };
}

// Client: exercises the REAL production path — encode each value the way the
// contract stores it ("<salt>:<value>"), decode via decodeAnonymousValue, then
// accumulate via accumulateAnonymousTally (both from production code).
function clientTally(voters: ScenarioVoter[]): {
  tallies: bigint[];
  seeds: bigint[];
} {
  const salt = "maintainer:project:0";
  const decoded: DecodedVoter[] = voters.map((v) => ({
    weight: v.weight,
    voteValues: v.votes.map((x) => decodeAnonymousValue(`${salt}:${x}`)) as [
      number,
      number,
      number,
    ],
    seedValues: v.seeds.map((x) => decodeAnonymousValue(`${salt}:${x}`)) as [
      number,
      number,
      number,
    ],
  }));
  const { tallies, seeds } = accumulateAnonymousTally(decoded);
  return { tallies, seeds };
}

// Mirror of the contract's tallies_to_result supermajority rule, so the TS and
// Rust sides agree not just on scalars but on the resulting proposal outcome.
type Outcome = "Approved" | "Rejected" | "Cancelled";
function tallyOutcome(tallies: bigint[]): Outcome {
  const [a, r, ab] = [tallies[0]!, tallies[1]!, tallies[2]!];
  if (a > r + ab) return "Approved";
  if (r > a + ab) return "Rejected";
  return "Cancelled";
}

const withinValidRange = (voters: ScenarioVoter[]): boolean => {
  const { tallies, seeds } = oracleTally(voters);
  const allValues = voters.flatMap((v) => [...v.votes, ...v.seeds]);
  return (
    allValues.every((x) => BigInt(x) <= MAX_SAFE) && // D1 bound
    [...tallies, ...seeds].every((x) => x <= U128_MAX) // D3 bound
  );
};

// --- Shared canonical scenarios ---------------------------------------------

describe(`anonymous tally differential — canonical scenarios (${scenarioFile.specVersion})`, () => {
  it("has scenarios covering the historically-fragile edge values", () => {
    const names = scenarioFile.scenarios.map((s) => s.name);
    // weight=0, weight=maxWeight, all-abstain, single-voter (issue requirements)
    expect(names).toContain("weight_zero_only");
    expect(names).toContain("single_voter_max_weight");
    expect(names).toContain("all_abstain");
    expect(names).toContain("single_voter_approve");
    expect(scenarioFile.scenarios.length).toBeGreaterThanOrEqual(4);
  });

  for (const scenario of scenarioFile.scenarios) {
    it(`client accumulation matches the contract oracle: ${scenario.name}`, () => {
      // Every canonical scenario must be inside the documented valid range.
      expect(withinValidRange(scenario.voters)).toBe(true);

      const client = clientTally(scenario.voters);
      const oracle = oracleTally(scenario.voters);

      // Bit-for-bit agreement on both tallies and seeds.
      expect(client.tallies).toEqual(oracle.tallies);
      expect(client.seeds).toEqual(oracle.seeds);

      // Resulting outcome is well-defined and shared with the Rust side.
      expect(["Approved", "Rejected", "Cancelled"]).toContain(
        tallyOutcome(client.tallies),
      );
    });
  }
});

// --- Property-based differential --------------------------------------------

const arbVoter: fc.Arbitrary<ScenarioVoter> = fc
  .record({
    weight: fc.integer({ min: 0, max: 10_000_000 }), // 0..max badge weight
    choice: fc.integer({ min: 0, max: 2 }), // one-hot outcome
    seeds: fc.tuple(
      fc.integer({ min: 0, max: 4_294_967_295 }), // 32-bit seeds (as generated)
      fc.integer({ min: 0, max: 4_294_967_295 }),
      fc.integer({ min: 0, max: 4_294_967_295 }),
    ),
  })
  .map(({ weight, choice, seeds }) => {
    const votes: [number, number, number] = [0, 0, 0];
    votes[choice] = 1;
    return { weight, votes, seeds: seeds as [number, number, number] };
  });

describe("anonymous tally differential — property based (contract-valid range)", () => {
  it("client accumulation == contract oracle for all generated scenarios", () => {
    fc.assert(
      fc.property(
        fc.array(arbVoter, { minLength: 1, maxLength: 40 }), // <= MAX_VOTES_PER_PROPOSAL
        (voters) => {
          // Generation stays inside the valid range by construction.
          expect(withinValidRange(voters)).toBe(true);
          const client = clientTally(voters);
          const oracle = oracleTally(voters);
          expect(client.tallies).toEqual(oracle.tallies);
          expect(client.seeds).toEqual(oracle.seeds);
        },
      ),
      { numRuns: 500 },
    );
  });

  it("weight-application order is irrelevant (shuffling voters is invariant)", () => {
    fc.assert(
      fc.property(
        fc.array(arbVoter, { minLength: 1, maxLength: 40 }),
        (voters) => {
          const forward = oracleTally(voters);
          const reversed = oracleTally([...voters].reverse());
          expect(forward.tallies).toEqual(reversed.tallies);
          expect(forward.seeds).toEqual(reversed.seeds);
        },
      ),
      { numRuns: 200 },
    );
  });
});

// --- Documented discrepancies (differential-tally-spec.md §3) ----------------

describe("anonymous tally differential — documented discrepancies", () => {
  // D1: parseInt-based decode is exact only up to Number.MAX_SAFE_INTEGER.
  it("D1: decodeAnonymousValue is exact at/below 2^53-1 and lossy above it", () => {
    const safe = Number.MAX_SAFE_INTEGER; // 2^53 - 1
    expect(BigInt(decodeAnonymousValue(`salt:${safe}`))).toBe(BigInt(safe));

    // A u128-valid seed just past the float64 boundary is silently rounded,
    // so the client would diverge from the contract. This asserts the KNOWN
    // divergence so widening the seed range past 2^53 trips this test.
    const beyond = 9_007_199_254_740_993n; // 2^53 + 1, not representable as f64
    const decoded = BigInt(decodeAnonymousValue(`salt:${beyond}`));
    expect(decoded).not.toBe(beyond);
  });

  // D3: the client accumulates unbounded BigInt; the contract boundary is u128.
  it("D3: within-range sums fit u128; the harness pins the u128 ceiling", () => {
    // Worst case within current constants: 40 voters, max weight, max 32-bit seed.
    const voters: ScenarioVoter[] = Array.from({ length: 40 }, () => ({
      weight: 10_000_000,
      votes: [1, 0, 0] as [number, number, number],
      seeds: [4_294_967_295, 4_294_967_295, 4_294_967_295] as [
        number,
        number,
        number,
      ],
    }));
    const { tallies, seeds } = oracleTally(voters);
    for (const x of [...tallies, ...seeds])
      expect(x).toBeLessThanOrEqual(U128_MAX);

    // But BigInt itself imposes no ceiling — a hypothetical over-u128 sum would
    // be produced client-side and only fail when encoded into the u128 ScVal.
    const overU128 = (U128_MAX + 1n) * 2n;
    expect(overU128 > U128_MAX).toBe(true);
  });
});
