// Pure, dependency-free anonymous-tally kernel.
//
// This is the single source of truth for the client-side scalar tally that
// `computeAnonymousVotingData` (anonymousVoting.ts) submits to the contract's
// `proof()`/`execute()`. It is deliberately free of Astro/SSR, network and
// crypto imports so it can be exercised directly by the cross-implementation
// differential harness (tests/differential/tallyDifferential.test.ts) against
// the real Soroban contract logic.
//
// The formula mirrors the contract exactly (see
// tests/differential/differential-tally-spec.md §1):
//
//     tally[i] = Σ_voters  weight · vote[i]
//     seed[i]  = Σ_voters  weight · seed[i]
//
// Accumulation uses BigInt so it is exact for any u128-range result; the
// contract performs the same integer sum inside the BLS commitment check.

export interface DecodedVoter {
  /** Voting weight actually applied to this voter's ballot (u32 on-chain). */
  weight: number;
  /** Decoded per-outcome vote values [approve, reject, abstain]. */
  voteValues: [number, number, number];
  /** Decoded per-outcome seed values [approve, reject, abstain]. */
  seedValues: [number, number, number];
}

export interface ScalarTally {
  /** Weighted vote tallies [approve, reject, abstain]. */
  tallies: [bigint, bigint, bigint];
  /** Weighted seed sums [approve, reject, abstain]. */
  seeds: [bigint, bigint, bigint];
  /** Un-weighted counts of ballots with a positive vote per outcome. */
  voteCounts: [number, number, number];
}

/**
 * Decode a decrypted (or plain) anonymous payload to its integer value.
 *
 * The encrypted plaintext is `"<salt>:<value>"`, so the trailing colon-segment
 * is the value; a plain `"0"`/`"1"` has no colon and is returned as-is.
 *
 * NOTE: `parseInt` returns a float64 and is therefore exact only for values
 * `<= Number.MAX_SAFE_INTEGER` (2^53 - 1). The contract accepts seeds up to
 * u128. This is discrepancy **D1** in differential-tally-spec.md — safe today
 * only because seeds are generated as 32-bit values.
 */
export function decodeAnonymousValue(plaintext: string): number {
  return parseInt(plaintext.split(":").pop()!);
}

/**
 * Accumulate the weighted scalar tally/seed sums from decoded voters.
 *
 * This is the exact accumulation performed by `computeAnonymousVotingData`,
 * extracted so production and the differential harness share one code path.
 */
export function accumulateAnonymousTally(voters: DecodedVoter[]): ScalarTally {
  const tallies: [bigint, bigint, bigint] = [0n, 0n, 0n];
  const seeds: [bigint, bigint, bigint] = [0n, 0n, 0n];
  const voteCounts: [number, number, number] = [0, 0, 0];

  for (const voter of voters) {
    const weight = BigInt(voter.weight);
    for (let i = 0; i < 3; i++) {
      const vote = voter.voteValues[i]!;
      const seed = voter.seedValues[i]!;
      tallies[i] += BigInt(vote) * weight;
      seeds[i] += BigInt(seed) * weight;
      if (vote > 0) voteCounts[i] += 1;
    }
  }

  return { tallies, seeds, voteCounts };
}
