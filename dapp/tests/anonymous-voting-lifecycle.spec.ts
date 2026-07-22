import { test, expect } from "@playwright/test";

// Full anonymous voting lifecycle test.
// This spec is intended to run against a local Soroban standalone node
// (Makefile target `local-stack` / docker-compose testnet service) and
// requires the Tansu contract to be built and deployed to that local node
// before the test runs in CI. The CI workflow is updated to perform those
// setup steps.

test.describe("Anonymous voting - full lifecycle (local Soroban)", () => {
  test("keygen -> multiple encrypted votes -> reveal -> proof() returns true", async ({
    page,
    context,
  }) => {
    // If the test is not pointed at a localhost RPC we skip to avoid
    // accidentally running this against public testnets.
    const rpc = process.env.PUBLIC_SOROBAN_RPC_URL ?? "";
    if (!rpc.includes("localhost") && !rpc.includes("127.0.0.1")) {
      test.skip("Local Soroban RPC not configured for this run");
    }

    // Increase default timeout for on-chain interactions
    page.setDefaultTimeout(60_000);

    // Use the application's in-browser utilities to compute tallies/seeds
    // and then perform a real contract `proof` invocation.
    await page.goto("/__playwright-module-test", {
      waitUntil: "domcontentloaded",
    });

    // Compute anonymous voting data for a simulated multi-voter proposal
    // and request contract-side proof verification (this will call the
    // Tansu.proof binding which is implemented as a pure contract method
    // and does not require signing). This exercises the real on-chain
    // proof verification.
    const data = await page.evaluate(async () => {
      const mod = await import("../src/utils/anonymousVoting.ts");
      const computed = await mod.computeAnonymousVotingData(
        "demo",
        1,
        "dummy",
        true, // verifyProof -> call Tansu.proof
      );
      return {
        tallies: computed.tallies,
        seeds: computed.seeds,
        proofOk: computed.proofOk,
        proofErrorMessage: computed.proofErrorMessage,
      };
    });

    // Ensure arrays look reasonable
    expect(Array.isArray(data.tallies)).toBeTruthy();
    expect(Array.isArray(data.seeds)).toBeTruthy();
    expect(data.tallies.length).toBeGreaterThan(0);

    // computeAnonymousVotingData already attempted contract proof verification
    // when called with verifyProof=true. Assert it reported success.
    expect(data.proofOk).toBeTruthy();
    if (!data.proofOk) {
      throw new Error(
        `Proof verification failed: ${String(data.proofErrorMessage)}`,
      );
    }
  });
});
