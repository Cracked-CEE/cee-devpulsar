//! Cross-implementation differential test for the anonymous tally.
//!
//! The TypeScript client (`dapp/src/utils/anonymousTally.ts`, used by
//! `computeAnonymousVotingData`) and this contract (`contract_dao.rs`
//! `proof`/`build_commitments_from_votes`) independently compute the same
//! weighted tally-and-verify. This test drives the **real contract** over the
//! **same shared scenarios** (`dapp/tests/differential/scenarios.json`) the
//! TypeScript suite (`tallyDifferential.test.ts`) uses, so the two languages are
//! validated against one source of truth.
//!
//! For each scenario it reproduces exactly the algebra `proof()` performs:
//!   * per-voter commitments are built by the real on-chain
//!     `build_commitments_from_votes`;
//!   * they are aggregated weighted: `Σ_voters weight · C_voter,i` (the loop in
//!     `proof()`); and
//!   * the client's scalar tally (`tally_i = Σ w·vote_i`, `seed_i = Σ w·seed_i`)
//!     is turned into the check commitment `g·tally_i + h·seed_i` — again via the
//!     real `build_commitments_from_votes`.
//!
//! Agreement (`assert_eq`) proves the client's scalars reproduce the contract's
//! aggregate; a perturbed tally must diverge (`assert_ne`).
//!
//! `tally_differential_proof_entrypoint` additionally casts real votes and calls
//! the literal `proof()` entrypoint, returning `true` for the client's scalars
//! and `false` for a perturbed one.
//!
//! See `dapp/tests/differential/differential-tally-spec.md` for the formula and
//! the re-validation process after a contract upgrade.

extern crate std;

use soroban_sdk::crypto::bls12_381::Bls12381G1Affine;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, BytesN, String as SorobanString, U256, Vec, vec};

use super::test_utils::{TestSetup, create_test_data, init_contract};
use crate::types::{AnonymousVote, Badge, Vote};

// Single source of truth, shared with the TypeScript differential suite.
static SCENARIOS_JSON: &str = include_str!("../../../../dapp/tests/differential/scenarios.json");

#[derive(serde::Deserialize)]
struct ScenarioFile {
    #[serde(rename = "specVersion")]
    spec_version: std::string::String,
    scenarios: std::vec::Vec<Scenario>,
}

#[derive(serde::Deserialize)]
struct Scenario {
    name: std::string::String,
    #[serde(default)]
    voters: std::vec::Vec<ScenarioVoter>,
}

#[derive(serde::Deserialize)]
struct ScenarioVoter {
    weight: u32,
    // u64 is plenty for the documented valid range (seeds are 32-bit); cast to u128.
    votes: [u64; 3],
    seeds: [u64; 3],
}

fn load_scenarios() -> ScenarioFile {
    serde_json::from_str(SCENARIOS_JSON).expect("scenarios.json must be valid JSON")
}

/// Identity point of G1, encoded the same way `proof()` seeds its accumulators.
fn g1_identity(setup: &TestSetup) -> Bls12381G1Affine {
    let mut bytes = [0u8; 96];
    bytes[0] = 0x40;
    Bls12381G1Affine::from_bytes(BytesN::from_array(&setup.env, &bytes))
}

fn u128_vec(setup: &TestSetup, values: &[u64; 3]) -> Vec<u128> {
    vec![
        &setup.env,
        values[0] as u128,
        values[1] as u128,
        values[2] as u128,
    ]
}

/// Reference scalar tally exactly as the client computes it:
/// `tally_i = Σ w·vote_i`, `seed_i = Σ w·seed_i`.
fn reference_tally(voters: &[ScenarioVoter]) -> ([u128; 3], [u128; 3]) {
    let mut tallies = [0u128; 3];
    let mut seeds = [0u128; 3];
    for voter in voters {
        for i in 0..3 {
            tallies[i] += voter.weight as u128 * voter.votes[i] as u128;
            seeds[i] += voter.weight as u128 * voter.seeds[i] as u128;
        }
    }
    (tallies, seeds)
}

#[test]
fn tally_differential_scenarios_match_contract_commitments() {
    let file = load_scenarios();
    assert_eq!(
        file.spec_version.as_str(),
        "tansu-2.0.2",
        "scenarios.json specVersion drifted from the contract version; \
         re-validate per differential-tally-spec.md §4",
    );
    assert!(
        file.scenarios.len() >= 4,
        "issue requires 4+ edge scenarios (weight=0, maxWeight, all-abstain, single-voter)",
    );

    for scenario in &file.scenarios {
        let setup = create_test_data();
        let project_key = init_contract(&setup);
        let public_key = SorobanString::from_str(&setup.env, "pk_differential");
        setup
            .contract
            .anonymous_voting_setup(&setup.mando, &project_key, &public_key);

        let bls = setup.env.crypto().bls12_381();

        // Aggregate the per-voter commitments, scaled by weight — this is the
        // exact loop `proof()` runs over the stored votes.
        let mut aggregate = [
            g1_identity(&setup),
            g1_identity(&setup),
            g1_identity(&setup),
        ];
        for voter in &scenario.voters {
            let commitments = setup.contract.build_commitments_from_votes(
                &project_key,
                &u128_vec(&setup, &voter.votes),
                &u128_vec(&setup, &voter.seeds),
            );
            let weight: U256 = U256::from_u32(&setup.env, voter.weight);
            for i in 0..3u32 {
                let commitment = Bls12381G1Affine::from_bytes(commitments.get(i).unwrap());
                let weighted = bls.g1_mul(&commitment, &weight.clone().into());
                aggregate[i as usize] = bls.g1_add(&aggregate[i as usize], &weighted);
            }
        }

        // The client's scalar tally, turned into the check commitment
        // `g·tally_i + h·seed_i` via the real contract builder.
        let (tallies, seeds) = reference_tally(&scenario.voters);
        let checks = setup.contract.build_commitments_from_votes(
            &project_key,
            &vec![&setup.env, tallies[0], tallies[1], tallies[2]],
            &vec![&setup.env, seeds[0], seeds[1], seeds[2]],
        );

        for i in 0..3u32 {
            let check = Bls12381G1Affine::from_bytes(checks.get(i).unwrap());
            assert_eq!(
                aggregate[i as usize], check,
                "scenario '{}' outcome {} — client scalar tally diverges from the \
                 contract's aggregate commitment",
                scenario.name, i,
            );
        }

        // Negative control: a tally off by one must NOT reproduce the aggregate.
        let mut perturbed = tallies;
        perturbed[0] += 1;
        let bad_checks = setup.contract.build_commitments_from_votes(
            &project_key,
            &vec![&setup.env, perturbed[0], perturbed[1], perturbed[2]],
            &vec![&setup.env, seeds[0], seeds[1], seeds[2]],
        );
        let bad_check = Bls12381G1Affine::from_bytes(bad_checks.get(0).unwrap());
        assert_ne!(
            aggregate[0], bad_check,
            "scenario '{}' — perturbed tally unexpectedly matched (harness would \
             not catch a real divergence)",
            scenario.name,
        );
    }
}

/// End-to-end coverage of the literal `proof()` entrypoint with real cast votes.
///
/// Mirrors `test_dao::dao_anonymous`'s setup: the proposer is auto-added to the
/// abstain group (weight 0), then real voters cast anonymous ballots. `proof()`
/// must accept the client's `Σ w·vote` / `Σ w·seed` and reject a perturbation.
#[test]
fn tally_differential_proof_entrypoint() {
    let setup = create_test_data();
    let project_key = init_contract(&setup);
    let public_key = SorobanString::from_str(&setup.env, "pk_differential_e2e");
    setup
        .contract
        .anonymous_voting_setup(&setup.mando, &project_key, &public_key);

    let title = SorobanString::from_str(&setup.env, "Differential proof() coverage");
    let ipfs = SorobanString::from_str(
        &setup.env,
        "bafybeib6ioupho3p3pliusx7tgs7dvi6mpu2bwfhayj6w6ie44lo3vvc4i",
    );
    let voting_ends_at = setup.env.ledger().timestamp() + 3600 * 24 * 2;
    let proposal_id = setup.contract.create_proposal(
        &setup.grogu,
        &project_key,
        &title,
        &ipfs,
        &voting_ends_at,
        &false, // anonymous
        &None,
        &None,
    );

    // Two real voters with elevated badges so their chosen weights are allowed.
    // Voter A: approve, weight 7, seeds [101,0,0]; Voter B: reject, weight 2.
    let voters: [(Address, u32, [u128; 3], [u128; 3]); 2] = [
        (Address::generate(&setup.env), 7, [1, 0, 0], [101, 0, 0]),
        (Address::generate(&setup.env), 2, [0, 1, 0], [0, 55, 0]),
    ];

    for (address, weight, vote_values, seed_values) in &voters {
        setup.token_stellar.mint(address, &(10 * 10_000_000));
        setup
            .contract
            .add_member(address, &SorobanString::from_str(&setup.env, "meta"));
        setup.contract.set_badges(
            &setup.mando,
            &project_key,
            address,
            &vec![&setup.env, Badge::Developer], // 10_000_000 weight ceiling
        );

        let commitments = setup.contract.build_commitments_from_votes(
            &project_key,
            &vec![&setup.env, vote_values[0], vote_values[1], vote_values[2]],
            &vec![&setup.env, seed_values[0], seed_values[1], seed_values[2]],
        );
        let placeholder = vec![
            &setup.env,
            SorobanString::from_str(&setup.env, "enc"),
            SorobanString::from_str(&setup.env, "enc"),
            SorobanString::from_str(&setup.env, "enc"),
        ];
        let vote = Vote::AnonymousVote(AnonymousVote {
            address: address.clone(),
            weight: *weight,
            encrypted_seeds: placeholder.clone(),
            encrypted_votes: placeholder,
            commitments,
        });
        setup
            .contract
            .vote(address, &project_key, &proposal_id, &vote);
    }

    // Client scalars: proposer's weight-0 abstain contributes nothing.
    let tallies = vec![&setup.env, 7u128, 2u128, 0u128];
    let seeds = vec![&setup.env, 707u128, 110u128, 0u128];

    let proposal = setup.contract.get_proposal(&project_key, &proposal_id);
    assert!(
        setup
            .contract
            .proof(&project_key, &proposal, &tallies, &seeds),
        "proof() must accept the client's Σ w·vote / Σ w·seed scalars",
    );

    let bad_tallies = vec![&setup.env, 8u128, 2u128, 0u128];
    assert!(
        !setup
            .contract
            .proof(&project_key, &proposal, &bad_tallies, &seeds),
        "proof() must reject a perturbed tally",
    );
}
