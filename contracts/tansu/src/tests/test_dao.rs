use super::test_utils::{create_test_data, init_contract};
use crate::contract_dao;
use crate::events::{AnonymousVotingSetup, ProposalCreated, ProposalExecuted, VoteCast};
use crate::{
    errors::ContractErrors,
    types::{
        AnonymousVote, Badge, Dao, OutcomeContract, ProposalStatus, PublicVote, TIMELOCK_DELAY,
        Vote, VoteChoice,
    },
};
use soroban_sdk::testutils::{Address as _, Events, Ledger};
use soroban_sdk::{
    Address, BytesN, Env, Event, IntoVal, String, Symbol, contract, contractimpl, vec,
};

#[contract]
pub struct TestOutcomeContract;

#[contractimpl]
impl TestOutcomeContract {
    pub fn execute_approve(_env: Env, maintainer: Address, _value: u32) {
        maintainer.require_auth();
    }

    pub fn execute_reject(_env: Env, _maintainer: Address, _value: u32) {}

    pub fn execute_cancel(_env: Env, _maintainer: Address, _value: u32) {}
}

#[test]
fn proposal_flow() {
    let setup = create_test_data();
    let id = init_contract(&setup);

    let title = String::from_str(&setup.env, "Integrate with xlm.sh");
    let ipfs = String::from_str(
        &setup.env,
        "bafybeib6ioupho3p3pliusx7tgs7dvi6mpu2bwfhayj6w6ie44lo3vvc4i",
    );
    let voting_ends_at = setup.env.ledger().timestamp() + 3600 * 24 * 2;

    let balance_proposer_init = setup.token_stellar.balance(&setup.grogu);
    let balance_voter_init = setup.token_stellar.balance(&setup.mando);

    let proposal_id = setup.contract.create_proposal(
        &setup.grogu,
        &id,
        &title,
        &ipfs,
        &voting_ends_at,
        &true,
        &None,
        &None,
    );

    // Verify proposal creation event
    let event = ProposalCreated {
        project_key: id.clone(),
        proposal_id,
        title: title.clone(),
        proposer: setup.grogu.clone(),
        voting_ends_at,
        public_voting: true,
        token_contract: None,
    };

    let contract_events = setup
        .env
        .events()
        .all()
        .filter_by_contract(&setup.contract_id);

    assert_eq!(
        contract_events,
        [event.to_xdr(&setup.env, &setup.contract_id)]
    );

    let balance_proposer_ = setup.token_stellar.balance(&setup.grogu);
    assert!(balance_proposer_init > balance_proposer_);

    setup.contract.vote(
        &setup.mando,
        &id,
        &proposal_id,
        &Vote::PublicVote(PublicVote {
            address: setup.mando.clone(),
            weight: 1,
            vote_choice: VoteChoice::Approve,
        }),
    );

    // Verify vote cast event
    let event = VoteCast {
        project_key: id.clone(),
        proposal_id,
        voter: setup.mando.clone(),
    };

    let contract_events = setup
        .env
        .events()
        .all()
        .filter_by_contract(&setup.contract_id);
    assert_eq!(
        contract_events,
        [event.to_xdr(&setup.env, &setup.contract_id)]
    );

    let balance_voter_ = setup.token_stellar.balance(&setup.mando);
    // Exact amount, not just "some collateral was locked": pins VOTE_COLLATERAL
    // (2 * 10_000_000) so a corrupted constant is caught, not just its sign.
    assert_eq!(balance_voter_init - balance_voter_, 2 * 10_000_000);

    setup
        .env
        .ledger()
        .set_timestamp(voting_ends_at + TIMELOCK_DELAY + 1);
    let result = setup
        .contract
        .execute(&setup.mando, &id, &proposal_id, &None, &None);

    // Verify proposal executed event
    let event = ProposalExecuted {
        project_key: id.clone(),
        proposal_id,
        status: String::from_str(&setup.env, "Approved"),
        maintainer: setup.mando.clone(),
    };

    let contract_events = setup
        .env
        .events()
        .all()
        .filter_by_contract(&setup.contract_id);

    assert_eq!(
        contract_events,
        [event.to_xdr(&setup.env, &setup.contract_id)]
    );

    assert_eq!(result, ProposalStatus::Approved);

    let balance_proposer_ = setup.token_stellar.balance(&setup.grogu);
    assert_eq!(balance_proposer_init, balance_proposer_);

    let balance_voter_ = setup.token_stellar.balance(&setup.mando);
    assert_eq!(balance_voter_init, balance_voter_);
}

#[test]
fn scf_voting() {
    let setup = create_test_data();
    init_contract(&setup);

    // Stellarpg project for SCF voting
    let name = String::from_str(&setup.env, "stellarpg");
    let url = String::from_str(&setup.env, "github.com/tansu");
    let ipfs = String::from_str(&setup.env, "2ef4f49fdd8fa9dc463f1f06a094c26b88710990");
    let maintainers = vec![&setup.env, setup.grogu.clone(), setup.mando.clone()];
    let id = setup
        .contract
        .register(&setup.grogu, &name, &maintainers, &url, &ipfs);

    let title = String::from_str(&setup.env, "A SCF proposal");
    let ipfs = String::from_str(
        &setup.env,
        "bafybeib6ioupho3p3pliusx7tgs7dvi6mpu2bwfhayj6w6ie44lo3vvc4i",
    );
    let voting_ends_at = setup.env.ledger().timestamp() + 3600 * 24 * 2;
    let proposal_id = setup.contract.create_proposal(
        &setup.grogu,
        &id,
        &title,
        &ipfs,
        &voting_ends_at,
        &true,
        &None,
        &None,
    );

    setup.contract.vote(
        &setup.mando,
        &id,
        &proposal_id,
        &Vote::PublicVote(PublicVote {
            address: setup.mando.clone(),
            weight: 10_000_000,
            vote_choice: VoteChoice::Approve,
        }),
    );

    setup
        .env
        .ledger()
        .set_timestamp(voting_ends_at + TIMELOCK_DELAY + 1);
    let result = setup
        .contract
        .execute(&setup.mando, &id, &proposal_id, &None, &None);

    assert_eq!(result, ProposalStatus::Approved);
}

#[test]
fn dao_basic_functionality() {
    let setup = create_test_data();
    let id = init_contract(&setup);

    // Test empty DAO initially
    let dao = setup.contract.get_dao(&id, &0);
    assert_eq!(
        dao,
        Dao {
            proposals: vec![&setup.env]
        }
    );

    let title = String::from_str(&setup.env, "Integrate with xlm.sh");
    let ipfs = String::from_str(
        &setup.env,
        "bafybeib6ioupho3p3pliusx7tgs7dvi6mpu2bwfhayj6w6ie44lo3vvc4i",
    );
    let voting_ends_at = setup.env.ledger().timestamp() + 3600 * 24 * 2;

    let proposal_id = setup.contract.create_proposal(
        &setup.grogu,
        &id,
        &title,
        &ipfs,
        &voting_ends_at,
        &true,
        &None,
        &None,
    );
    assert_eq!(proposal_id, 0);

    let proposal = setup.contract.get_proposal(&id, &proposal_id);
    assert_eq!(proposal.id, 0);
    assert_eq!(proposal.title, title);
    assert_eq!(proposal.ipfs, ipfs);
    assert_eq!(proposal.vote_data.voting_ends_at, voting_ends_at);

    let dao = setup.contract.get_dao(&id, &0);
    assert_eq!(dao.proposals.len(), 1);
    let mut expected_stored = proposal.clone();
    expected_stored.vote_data.votes = vec![&setup.env];
    assert_eq!(dao.proposals.get(0), Some(expected_stored));
}

#[test]
fn dao_anonymous() {
    let setup = create_test_data();
    let id = init_contract(&setup);

    let title = String::from_str(&setup.env, "Integrate with xlm.sh");
    let ipfs = String::from_str(
        &setup.env,
        "bafybeib6ioupho3p3pliusx7tgs7dvi6mpu2bwfhayj6w6ie44lo3vvc4i",
    );
    let voting_ends_at = setup.env.ledger().timestamp() + 3600 * 24 * 2;

    let public_key = String::from_str(&setup.env, "public key random");
    setup
        .contract
        .anonymous_voting_setup(&setup.mando, &id, &public_key);

    let event = AnonymousVotingSetup {
        project_key: id.clone(),
        maintainer: setup.mando.clone(),
        public_key: public_key.clone(),
    };

    let all_events = setup
        .env
        .events()
        .all()
        .filter_by_contract(&setup.contract_id);
    assert_eq!(all_events, [event.to_xdr(&setup.env, &setup.contract_id)]);

    // Add a member with elevated rights
    let kuiil = Address::generate(&setup.env);
    setup.token_stellar.mint(&kuiil, &(10 * 10_000_000));
    let meta = String::from_str(&setup.env, "abcd");
    setup.contract.add_member(&kuiil, &meta);
    let badges = vec![&setup.env, Badge::Community];
    setup
        .contract
        .set_badges(&setup.mando, &id, &kuiil, &badges);

    let proposal_id = setup.contract.create_proposal(
        &setup.grogu,
        &id,
        &title,
        &ipfs,
        &voting_ends_at,
        &false,
        &None,
        &None,
    );
    assert_eq!(proposal_id, 0);

    let proposal = setup.contract.get_proposal(&id, &proposal_id);

    // test build_commitments_from_votes and abstain
    let abstain_vote = Vote::AnonymousVote(AnonymousVote {
        address: setup.grogu.clone(),
        weight: 0u32,
        encrypted_seeds: vec![
            &setup.env,
            String::from_str(&setup.env, "0"),
            String::from_str(&setup.env, "0"),
            String::from_str(&setup.env, "0"),
        ],
        encrypted_votes: vec![
            &setup.env,
            String::from_str(&setup.env, "0"),
            String::from_str(&setup.env, "0"),
            String::from_str(&setup.env, "1"),
        ],
        commitments: setup.contract.build_commitments_from_votes(
            &id,
            &vec![&setup.env, 0u128, 0u128, 1u128],
            &vec![&setup.env, 0u128, 0u128, 0u128],
        ),
    });

    assert_eq!(
        proposal.vote_data.votes,
        vec![&setup.env, abstain_vote.clone()]
    );

    let vote_ = Vote::AnonymousVote(AnonymousVote {
        address: kuiil.clone(),
        weight: 3,
        encrypted_seeds: vec![
            &setup.env,
            String::from_str(&setup.env, "fafdas"),
            String::from_str(&setup.env, "fafdas"),
            String::from_str(&setup.env, "fafdas"),
        ],
        encrypted_votes: vec![
            &setup.env,
            String::from_str(&setup.env, "fafdas"),
            String::from_str(&setup.env, "fafdas"),
            String::from_str(&setup.env, "rewrewr"),
        ],
        commitments: setup.contract.build_commitments_from_votes(
            &id,
            &vec![&setup.env, 3u128, 1u128, 1u128],
            &vec![&setup.env, 5u128, 4u128, 6u128],
        ),
    });
    setup.contract.vote(&kuiil, &id, &proposal_id, &vote_);

    setup
        .env
        .ledger()
        .set_timestamp(voting_ends_at + TIMELOCK_DELAY + 1);

    let vote_result = setup.contract.execute(
        &setup.grogu,
        &id,
        &proposal_id,
        &Some(vec![&setup.env, 9u128, 3u128, 3u128]),
        &Some(vec![&setup.env, 15u128, 12u128, 18u128]),
    );

    assert_eq!(vote_result, ProposalStatus::Approved);
}

#[test]
fn voting_errors() {
    let setup = create_test_data();
    let id = init_contract(&setup);

    let title = String::from_str(&setup.env, "Test Proposal");
    let ipfs = String::from_str(
        &setup.env,
        "bafybeib6ioupho3p3pliusx7tgs7dvi6mpu2bwfhayj6w6ie44lo3vvc4i",
    );
    let voting_ends_at = setup.env.ledger().timestamp() + 3600 * 24 * 2;

    let proposal_id = setup.contract.create_proposal(
        &setup.grogu,
        &id,
        &title,
        &ipfs,
        &voting_ends_at,
        &true,
        &None,
        &None,
    );

    // Start testing bad behaviours

    // Not initialized private voting
    let error = setup
        .contract
        .try_create_proposal(
            &setup.grogu,
            &id,
            &title,
            &ipfs,
            &voting_ends_at,
            &false,
            &None,
            &None,
        )
        .unwrap_err()
        .unwrap();
    assert_eq!(error, ContractErrors::NoAnonymousVotingConfig.into());

    let public_key = String::from_str(&setup.env, "public key random");
    setup
        .contract
        .anonymous_voting_setup(&setup.mando, &id, &public_key);

    let proposal_id_anonymous = setup.contract.create_proposal(
        &setup.grogu,
        &id,
        &title,
        &ipfs,
        &voting_ends_at,
        &false,
        &None,
        &None,
    );

    // Wrong vote type anonymous vs public
    let err = setup
        .contract
        .try_vote(
            &setup.mando,
            &id,
            &proposal_id_anonymous,
            &Vote::PublicVote(PublicVote {
                address: setup.mando.clone(),
                weight: 1,
                vote_choice: VoteChoice::Approve,
            }),
        )
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::WrongVoteType.into());

    // Cannot vote for your own proposal (creator automatically abstains)
    let err = setup
        .contract
        .try_vote(
            &setup.grogu,
            &id,
            &proposal_id,
            &Vote::PublicVote(PublicVote {
                address: setup.grogu.clone(),
                weight: 1,
                vote_choice: VoteChoice::Approve,
            }),
        )
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::AlreadyVoted.into());

    // Sending wrong commitments
    let error = setup
        .contract
        .try_vote(
            &setup.mando,
            &id,
            &proposal_id_anonymous,
            &Vote::AnonymousVote(AnonymousVote {
                address: setup.mando.clone(),
                weight: 0u32,
                encrypted_seeds: vec![&setup.env, String::from_str(&setup.env, "abcd")],
                encrypted_votes: vec![&setup.env, String::from_str(&setup.env, "fsfds")],
                commitments: vec![
                    &setup.env,
                    BytesN::from_array(&setup.env, &[0; 96]),
                    BytesN::from_array(&setup.env, &[0; 96]),
                ],
            }),
        )
        .unwrap_err()
        .unwrap();
    assert_eq!(error, ContractErrors::BadCommitment.into());

    // Vote successfully with another user
    setup.contract.vote(
        &setup.mando,
        &id,
        &proposal_id,
        &Vote::PublicVote(PublicVote {
            address: setup.mando.clone(),
            weight: 1,
            vote_choice: VoteChoice::Approve,
        }),
    );

    // Cannot vote twice
    let err = setup
        .contract
        .try_vote(
            &setup.mando,
            &id,
            &proposal_id,
            &Vote::PublicVote(PublicVote {
                address: setup.mando.clone(),
                weight: 1,
                vote_choice: VoteChoice::Approve,
            }),
        )
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::AlreadyVoted.into());

    // Cannot vote for someone else
    let kuiil = Address::generate(&setup.env);
    let meta = String::from_str(&setup.env, "test");
    setup.contract.add_member(&kuiil, &meta);

    let err = setup
        .contract
        .try_vote(
            &kuiil,
            &id,
            &proposal_id,
            &Vote::PublicVote(PublicVote {
                address: setup.mando.clone(),
                weight: 1,
                vote_choice: VoteChoice::Approve,
            }),
        )
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::WrongVoter.into());

    // Non-existent proposal
    let err = setup
        .contract
        .try_get_proposal(&id, &10)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::NoProposalOrPageFound.into());

    // Too early to execute
    let err = setup
        .contract
        .try_execute(&setup.mando, &id, &proposal_id, &None, &None)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::ProposalVotingTime.into());

    // advance time to allow execution
    setup
        .env
        .ledger()
        .set_timestamp(voting_ends_at + TIMELOCK_DELAY + 1);

    // Using args for anonymous voting in public votes. And the other way
    let err = setup
        .contract
        .try_execute(
            &setup.mando,
            &id,
            &proposal_id,
            &Some(vec![&setup.env, 9u128, 3u128, 500003u128]),
            &None,
        )
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::TallySeedError.into());

    let err = setup
        .contract
        .try_execute(&setup.mando, &id, &proposal_id_anonymous, &None, &None)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::TallySeedError.into());

    // Wrong tallies for proof
    let err = setup
        .contract
        .try_execute(
            &setup.grogu,
            &id,
            &proposal_id_anonymous,
            &Some(vec![&setup.env, 0u128, 0u128, 500_001u128]), // 500_000u128
            &Some(vec![&setup.env, 0u128, 0u128, 0u128]),
        )
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::InvalidProof.into());

    // Execute proposal for real
    setup
        .contract
        .execute(&setup.mando, &id, &proposal_id, &None, &None);

    // Already executed
    let err = setup
        .contract
        .try_execute(&setup.mando, &id, &proposal_id, &None, &None)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::ProposalActive.into());
}

#[test]
fn proposal_execution() {
    let setup = create_test_data();
    let id = init_contract(&setup);

    setup.env.ledger().set_timestamp(1234567890);
    let voting_ends_at = 1234567890 + 3600 * 24 * 2;

    let title = String::from_str(&setup.env, "Test Proposal");
    let ipfs = String::from_str(
        &setup.env,
        "bafybeib6ioupho3p3pliusx7tgs7dvi6mpu2bwfhayj6w6ie44lo3vvc4i",
    );

    let proposal_id = setup.contract.create_proposal(
        &setup.grogu,
        &id,
        &title,
        &ipfs,
        &voting_ends_at,
        &true,
        &None,
        &None,
    );

    // Add member with badge
    let kuiil = Address::generate(&setup.env);
    setup.token_stellar.mint(&kuiil, &(10 * 10_000_000));
    let meta = String::from_str(&setup.env, "test");
    setup.contract.add_member(&kuiil, &meta);
    let badges = vec![&setup.env, Badge::Community];
    setup
        .contract
        .set_badges(&setup.mando, &id, &kuiil, &badges);

    setup.contract.vote(
        &setup.mando,
        &id,
        &proposal_id,
        &Vote::PublicVote(PublicVote {
            address: setup.mando.clone(),
            weight: 1,
            vote_choice: VoteChoice::Approve,
        }),
    );

    setup.contract.vote(
        &kuiil,
        &id,
        &proposal_id,
        &Vote::PublicVote(PublicVote {
            address: kuiil.clone(),
            weight: Badge::Community as u32,
            vote_choice: VoteChoice::Approve,
        }),
    );

    setup
        .env
        .ledger()
        .set_timestamp(voting_ends_at + TIMELOCK_DELAY + 1);

    let vote_result = setup
        .contract
        .execute(&setup.mando, &id, &proposal_id, &None, &None);
    assert_eq!(vote_result, ProposalStatus::Approved);

    let proposal = setup.contract.get_proposal(&id, &proposal_id);
    assert_eq!(proposal.status, ProposalStatus::Approved);
}

#[test]
fn proposal_revoke() {
    let setup = create_test_data();
    let id = init_contract(&setup);

    setup.env.ledger().set_timestamp(1234567890);
    let voting_ends_at = 1234567890 + 3600 * 24 * 2;

    let title = String::from_str(&setup.env, "Test Proposal");
    let ipfs = String::from_str(
        &setup.env,
        "bafybeib6ioupho3p3pliusx7tgs7dvi6mpu2bwfhayj6w6ie44lo3vvc4i",
    );

    let proposal_id = setup.contract.create_proposal(
        &setup.grogu,
        &id,
        &title,
        &ipfs,
        &voting_ends_at,
        &true,
        &None,
        &None,
    );

    let kuiil = Address::generate(&setup.env);
    let err = setup
        .contract
        .try_revoke_proposal(&kuiil, &id, &proposal_id)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::UnauthorizedSigner.into());

    setup
        .contract
        .revoke_proposal(&setup.mando, &id, &proposal_id);

    let event = ProposalExecuted {
        project_key: id.clone(),
        proposal_id,
        status: String::from_str(&setup.env, "Malicious"),
        maintainer: setup.mando.clone(),
    };

    let all_events = setup
        .env
        .events()
        .all()
        .filter_by_contract(&setup.contract_id);
    assert_eq!(all_events, [event.to_xdr(&setup.env, &setup.contract_id)]);

    let proposal = setup.contract.get_proposal(&id, &proposal_id);
    assert_eq!(proposal.title, String::from_str(&setup.env, "REDACTED"));
    assert_eq!(proposal.ipfs, String::from_str(&setup.env, "NONE"));
    assert_eq!(proposal.status, ProposalStatus::Malicious);

    // already revoked and also try to call as an admin should go through the
    // auth part and fail later
    let err = setup
        .contract
        .try_revoke_proposal(&setup.contract_admin, &id, &proposal_id)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::ProposalActive.into());
}

#[test]
fn voter_weight_validation() {
    let setup = create_test_data();
    let id = init_contract(&setup);

    setup.env.ledger().set_timestamp(1234567890);
    let voting_ends_at = 1234567890 + 3600 * 24 * 2;

    let title = String::from_str(&setup.env, "Test Proposal");
    let ipfs = String::from_str(
        &setup.env,
        "bafybeib6ioupho3p3pliusx7tgs7dvi6mpu2bwfhayj6w6ie44lo3vvc4i",
    );

    let proposal_id = setup.contract.create_proposal(
        &setup.mando,
        &id,
        &title,
        &ipfs,
        &voting_ends_at,
        &true,
        &None,
        &None,
    );

    let kuiil = Address::generate(&setup.env);
    setup.token_stellar.mint(&kuiil, &(10 * 10_000_000));
    let meta = String::from_str(&setup.env, "test");
    setup.contract.add_member(&kuiil, &meta);

    // Cannot vote with weight higher than max allowed
    let err = setup
        .contract
        .try_vote(
            &kuiil,
            &id,
            &proposal_id,
            &Vote::PublicVote(PublicVote {
                address: kuiil.clone(),
                weight: u32::MAX,
                vote_choice: VoteChoice::Approve,
            }),
        )
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::VoterWeight.into());

    // Add developer badge and test reduced weight voting
    let badges = vec![&setup.env, Badge::Developer, Badge::Community];
    setup
        .contract
        .set_badges(&setup.mando, &id, &kuiil, &badges);

    let max_weight = setup.contract.get_max_weight(&id, &kuiil);
    assert_eq!(max_weight, 11_000_000u32);

    // Vote with reduced weight (should work)
    setup.contract.vote(
        &kuiil,
        &id,
        &proposal_id,
        &Vote::PublicVote(PublicVote {
            address: kuiil.clone(),
            weight: 42, // kuiil has up to 11M
            vote_choice: VoteChoice::Approve,
        }),
    );

    let proposal = setup.contract.get_proposal(&id, &proposal_id);
    let votes = &proposal.vote_data.votes;

    // Find kuiil's vote
    let kuiil_vote = votes.iter().find(|vote| {
        if let Vote::PublicVote(public_vote) = vote {
            public_vote.address == kuiil
        } else {
            false
        }
    });

    assert!(kuiil_vote.is_some());
    if let Some(Vote::PublicVote(public_vote)) = kuiil_vote {
        assert_eq!(public_vote.weight, 42);
        assert_eq!(public_vote.vote_choice, VoteChoice::Approve);
    }
}

#[test]
fn outcomes_execution() {
    let setup = create_test_data();
    let id = init_contract(&setup);

    // Register a simple test outcome contract
    let outcome_contract_id = setup.env.register(TestOutcomeContract, ());

    setup.env.ledger().set_timestamp(1234567890);
    let voting_ends_at = 1234567890 + 3600 * 24 * 2;

    let title = String::from_str(&setup.env, "Test Proposal");
    let ipfs = String::from_str(
        &setup.env,
        "bafybeib6ioupho3p3pliusx7tgs7dvi6mpu2bwfhayj6w6ie44lo3vvc4i",
    );

    // Create outcome contracts with approve and reject functions
    let approve_outcome = OutcomeContract {
        address: outcome_contract_id.clone(),
        execute_fn: Symbol::new(&setup.env, "execute_approve"),
        args: vec![
            &setup.env,
            setup.mando.clone().into_val(&setup.env),
            100u32.into_val(&setup.env),
        ],
    };

    let reject_outcome = OutcomeContract {
        address: outcome_contract_id.clone(),
        execute_fn: Symbol::new(&setup.env, "execute_reject"),
        args: vec![
            &setup.env,
            setup.mando.clone().into_val(&setup.env),
            200u32.into_val(&setup.env),
        ],
    };

    let outcome_contracts = vec![
        &setup.env,
        approve_outcome,
        reject_outcome,
        OutcomeContract {
            address: Address::generate(&setup.env), // dummy for abstain
            execute_fn: Symbol::new(&setup.env, "dummy"),
            args: vec![&setup.env],
        },
    ];

    let proposal_id = setup.contract.create_proposal(
        &setup.grogu,
        &id,
        &title,
        &ipfs,
        &voting_ends_at,
        &true,
        &None,
        &Some(outcome_contracts),
    );

    // Add member with badge
    let kuiil = Address::generate(&setup.env);
    setup.token_stellar.mint(&kuiil, &(10 * 10_000_000));
    let meta = String::from_str(&setup.env, "test");
    setup.contract.add_member(&kuiil, &meta);
    let badges = vec![&setup.env, Badge::Community];
    setup
        .contract
        .set_badges(&setup.mando, &id, &kuiil, &badges);

    setup.contract.vote(
        &setup.mando,
        &id,
        &proposal_id,
        &Vote::PublicVote(PublicVote {
            address: setup.mando.clone(),
            weight: 1,
            vote_choice: VoteChoice::Approve,
        }),
    );

    setup.contract.vote(
        &kuiil,
        &id,
        &proposal_id,
        &Vote::PublicVote(PublicVote {
            address: kuiil.clone(),
            weight: Badge::Community as u32,
            vote_choice: VoteChoice::Approve,
        }),
    );

    setup
        .env
        .ledger()
        .set_timestamp(voting_ends_at + TIMELOCK_DELAY + 1);

    let vote_result = setup
        .contract
        .execute(&setup.mando, &id, &proposal_id, &None, &None);
    assert_eq!(vote_result, ProposalStatus::Approved);

    let proposal = setup.contract.get_proposal(&id, &proposal_id);
    assert_eq!(proposal.status, ProposalStatus::Approved);
}

#[test]
fn token_based_proposal_flow() {
    let setup = create_test_data();
    let id = init_contract(&setup);

    let title = String::from_str(&setup.env, "Token-based voting test");
    let ipfs = String::from_str(
        &setup.env,
        "bafybeib6ioupho3p3pliusx7tgs7dvi6mpu2bwfhayj6w6ie44lo3vvc4i",
    );
    let voting_ends_at = setup.env.ledger().timestamp() + 3600 * 24 * 2;

    // Record initial balances
    let balance_proposer_init = setup.token_stellar.balance(&setup.grogu);
    let balance_voter_init = setup.token_stellar.balance(&setup.mando);

    // Create token-based proposal
    let proposal_id = setup.contract.create_proposal(
        &setup.grogu,
        &id,
        &title,
        &ipfs,
        &voting_ends_at,
        &true,                                      // public_voting
        &Some(setup.token_stellar.address.clone()), // token_contract
        &None,
    );

    // Verify proposal was created with token_contract
    let proposal = setup.contract.get_proposal(&id, &proposal_id);
    assert!(proposal.vote_data.token_contract.is_some());

    // Check proposer balance decreased by PROPOSAL_COLLATERAL only (no VOTE_COLLATERAL for token-based)
    let balance_proposer_after_create = setup.token_stellar.balance(&setup.grogu);
    let expected_proposer_deduction = 5 * 10_000_000; // PROPOSAL_COLLATERAL
    assert_eq!(
        balance_proposer_init - balance_proposer_after_create,
        expected_proposer_deduction
    );

    // Vote with weight based on token balance
    let vote_weight = 1000u32;
    setup.contract.vote(
        &setup.mando,
        &id,
        &proposal_id,
        &Vote::PublicVote(PublicVote {
            address: setup.mando.clone(),
            weight: vote_weight,
            vote_choice: VoteChoice::Approve,
        }),
    );

    // Verify voter balance decreased by vote_weight in whole tokens (scaled by decimals)
    let balance_voter_after_vote = setup.token_stellar.balance(&setup.mando);
    let token_scale = 10_000_000i128; // SAC default 7 decimals in tests
    let expected_deduction = vote_weight as i128 * token_scale;
    assert_eq!(
        balance_voter_init - balance_voter_after_vote,
        expected_deduction
    );

    // Move time forward to end voting period
    setup
        .env
        .ledger()
        .set_timestamp(voting_ends_at + TIMELOCK_DELAY + 1);

    // Execute the proposal
    let vote_result = setup
        .contract
        .execute(&setup.mando, &id, &proposal_id, &None, &None);
    assert_eq!(vote_result, ProposalStatus::Approved);

    // Verify balances were restored
    let balance_proposer_final = setup.token_stellar.balance(&setup.grogu);
    let balance_voter_final = setup.token_stellar.balance(&setup.mando);

    // Proposer gets back PROPOSAL_COLLATERAL (didn't pay VOTE_COLLATERAL for token-based)
    assert_eq!(balance_proposer_final, balance_proposer_init);

    // Voter gets back vote_weight in tokens (token WAS the collateral)
    assert_eq!(balance_voter_final, balance_voter_init);
}

#[test]
fn token_based_get_max_weight() {
    let setup = create_test_data();
    let id = init_contract(&setup);

    // For token-based proposals, get_max_weight returns badge-based weight
    // Token balance validation happens during the transfer in vote()
    let max_weight = setup.contract.get_max_weight(&id, &setup.mando);

    // Should return default badge weight (1) since no badges assigned
    assert_eq!(max_weight, 1);
}

#[test]
fn badge_based_get_max_weight_still_works() {
    let setup = create_test_data();
    let id = init_contract(&setup);

    // get_max_weight always returns badge-based weight
    let max_weight = setup.contract.get_max_weight(&id, &setup.mando);

    // Should return default badge weight (1) since no badges assigned
    assert_eq!(max_weight, 1);
}

#[test]
fn conflict_of_interest_flow() {
    use soroban_sdk::testutils::Address as _;

    let setup = create_test_data();
    let id = init_contract(&setup);

    let outsider = Address::generate(&setup.env);
    let genesis_amount: i128 = 1_000_000_000 * 10_000_000;
    setup.token_stellar.mint(&outsider, &genesis_amount);

    let title = String::from_str(&setup.env, "Conflict Proposal");
    let ipfs = String::from_str(
        &setup.env,
        "bafybeib6ioupho3p3pliusx7tgs7dvi6mpu2bwfhayj6w6ie44lo3vvc4i",
    );
    let voting_ends_at = setup.env.ledger().timestamp() + 3600 * 24 * 2;

    let proposal_id = setup.contract.create_proposal(
        &setup.grogu,
        &id,
        &title,
        &ipfs,
        &voting_ends_at,
        &true,
        &None,
        &None,
    );

    // Initially empty
    let list = setup.contract.get_conflict_of_interest(&id, &proposal_id);
    assert_eq!(list.len(), 0);

    // Non-maintainer cannot edit the list
    let err = setup
        .contract
        .try_add_conflict_of_interest(
            &outsider,
            &id,
            &proposal_id,
            &vec![&setup.env, outsider.clone()],
        )
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::UnauthorizedSigner.into());

    // Maintainer adds 2 addresses first
    let addr1 = outsider.clone();
    let addr2 = Address::generate(&setup.env);
    setup.contract.add_conflict_of_interest(
        &setup.mando,
        &id,
        &proposal_id,
        &vec![&setup.env, addr1.clone(), addr2.clone()],
    );
    let list = setup.contract.get_conflict_of_interest(&id, &proposal_id);
    assert_eq!(list.len(), 2);

    // Maintainer adds 3 more addresses, including a duplicate of addr1 which
    // must be ignored to keep the list de-duplicated
    let addr3 = Address::generate(&setup.env);
    let addr4 = Address::generate(&setup.env);
    let addr5 = Address::generate(&setup.env);
    setup.contract.add_conflict_of_interest(
        &setup.mando,
        &id,
        &proposal_id,
        &vec![
            &setup.env,
            addr1.clone(),
            addr3.clone(),
            addr4.clone(),
            addr5.clone(),
        ],
    );
    let list = setup.contract.get_conflict_of_interest(&id, &proposal_id);
    assert_eq!(list.len(), 5);
    assert_eq!(list.get(0).unwrap(), addr1);
    assert_eq!(list.get(1).unwrap(), addr2);
    assert_eq!(list.get(2).unwrap(), addr3);
    assert_eq!(list.get(3).unwrap(), addr4);
    assert_eq!(list.get(4).unwrap(), addr5);

    // Conflicted voter is blocked
    let err = setup
        .contract
        .try_vote(
            &outsider,
            &id,
            &proposal_id,
            &Vote::PublicVote(PublicVote {
                address: outsider.clone(),
                weight: 1,
                vote_choice: VoteChoice::Approve,
            }),
        )
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::VoterConflicted.into());

    // Maintainer removes the 2nd and 4th entries to exercise removal in the
    // middle of the list while preserving the rest
    setup.contract.remove_conflict_of_interest(
        &setup.mando,
        &id,
        &proposal_id,
        &vec![&setup.env, addr2.clone(), addr4.clone()],
    );
    let list = setup.contract.get_conflict_of_interest(&id, &proposal_id);
    assert_eq!(list.len(), 3);
    assert_eq!(list.get(0).unwrap(), addr1);
    assert_eq!(list.get(1).unwrap(), addr3);
    assert_eq!(list.get(2).unwrap(), addr5);

    // Maintainer removes addr1 so the previously-blocked voter can vote
    setup.contract.remove_conflict_of_interest(
        &setup.mando,
        &id,
        &proposal_id,
        &vec![&setup.env, addr1.clone()],
    );
    setup.contract.vote(
        &outsider,
        &id,
        &proposal_id,
        &Vote::PublicVote(PublicVote {
            address: outsider.clone(),
            weight: 1,
            vote_choice: VoteChoice::Approve,
        }),
    );

    // Non-maintainer cannot remove themselves
    setup.contract.add_conflict_of_interest(
        &setup.mando,
        &id,
        &proposal_id,
        &vec![&setup.env, outsider.clone()],
    );
    let err = setup
        .contract
        .try_remove_conflict_of_interest(
            &outsider,
            &id,
            &proposal_id,
            &vec![&setup.env, outsider.clone()],
        )
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::UnauthorizedSigner.into());
}

#[test]
fn remove_vote_public_flips_outcome() {
    let setup = create_test_data();
    let id = init_contract(&setup);

    let ipfs = String::from_str(
        &setup.env,
        "bafybeib6ioupho3p3pliusx7tgs7dvi6mpu2bwfhayj6w6ie44lo3vvc4i",
    );
    let voting_ends_at = setup.env.ledger().timestamp() + 3600 * 24 * 2;

    let proposal_id = setup.contract.create_proposal(
        &setup.grogu,
        &id,
        &String::from_str(&setup.env, "Test remove vote"),
        &ipfs,
        &voting_ends_at,
        &true,
        &None,
        &None,
    );

    // Auth check: outsider cannot remove a vote
    let outsider = Address::generate(&setup.env);
    assert_eq!(
        setup
            .contract
            .try_remove_vote(&outsider, &id, &proposal_id, &setup.mando),
        Err(Ok(ContractErrors::UnauthorizedSigner.into()))
    );

    // VoteNotFound check: cannot remove a vote that doesn't exist
    let non_voter = Address::generate(&setup.env);
    assert_eq!(
        setup
            .contract
            .try_remove_vote(&setup.grogu, &id, &proposal_id, &non_voter),
        Err(Ok(ContractErrors::VoteNotFound.into()))
    );

    // Give rex a Community badge so their vote carries more weight
    let rex = Address::generate(&setup.env);
    setup
        .token_stellar
        .mint(&rex, &(1_000_000_000 * 10_000_000));
    let meta = String::from_str(&setup.env, "rex");
    setup.contract.add_member(&rex, &meta);
    setup
        .contract
        .set_badges(&setup.mando, &id, &rex, &vec![&setup.env, Badge::Community]);

    // mando votes Approve (weight 1)
    setup.contract.vote(
        &setup.mando,
        &id,
        &proposal_id,
        &Vote::PublicVote(PublicVote {
            address: setup.mando.clone(),
            weight: 1,
            vote_choice: VoteChoice::Approve,
        }),
    );

    // rex votes Reject with high weight outcome would be Rejected without removal
    setup.contract.vote(
        &rex,
        &id,
        &proposal_id,
        &Vote::PublicVote(PublicVote {
            address: rex.clone(),
            weight: Badge::Community as u32,
            vote_choice: VoteChoice::Reject,
        }),
    );

    let balance_rex_after_vote = setup.token_stellar.balance(&rex);

    // Advance past voting period — removal must still work after deadline
    setup
        .env
        .ledger()
        .set_timestamp(voting_ends_at + TIMELOCK_DELAY + 1);

    // Maintainer removes rex's malicious reject vote
    setup
        .contract
        .remove_vote(&setup.grogu, &id, &proposal_id, &rex);

    // Rex's collateral is NOT returned — it was slashed as penalty
    assert_eq!(setup.token_stellar.balance(&rex), balance_rex_after_vote);

    let result = setup
        .contract
        .execute(&setup.grogu, &id, &proposal_id, &None, &None);
    assert_eq!(result, ProposalStatus::Approved);
}

#[test]
fn remove_vote_anonymous_flips_outcome() {
    let setup = create_test_data();
    let id = init_contract(&setup);

    let public_key = String::from_str(&setup.env, "public key random");
    setup
        .contract
        .anonymous_voting_setup(&setup.mando, &id, &public_key);

    let kuiil = Address::generate(&setup.env);
    setup.token_stellar.mint(&kuiil, &(10 * 10_000_000));
    setup
        .contract
        .add_member(&kuiil, &String::from_str(&setup.env, "kuiil"));
    setup.contract.set_badges(
        &setup.mando,
        &id,
        &kuiil,
        &vec![&setup.env, Badge::Community],
    );

    let rex = Address::generate(&setup.env);
    setup.token_stellar.mint(&rex, &(10 * 10_000_000));
    setup
        .contract
        .add_member(&rex, &String::from_str(&setup.env, "rex"));
    setup
        .contract
        .set_badges(&setup.mando, &id, &rex, &vec![&setup.env, Badge::Community]);

    let ipfs = String::from_str(
        &setup.env,
        "bafybeib6ioupho3p3pliusx7tgs7dvi6mpu2bwfhayj6w6ie44lo3vvc4i",
    );
    let voting_ends_at = setup.env.ledger().timestamp() + 3600 * 24 * 2;

    let proposal_id = setup.contract.create_proposal(
        &setup.grogu,
        &id,
        &String::from_str(&setup.env, "Anonymous remove vote test"),
        &ipfs,
        &voting_ends_at,
        &false,
        &None,
        &None,
    );

    let kuiil_vote = Vote::AnonymousVote(AnonymousVote {
        address: kuiil.clone(),
        weight: 3,
        encrypted_seeds: vec![
            &setup.env,
            String::from_str(&setup.env, "s0"),
            String::from_str(&setup.env, "s1"),
            String::from_str(&setup.env, "s2"),
        ],
        encrypted_votes: vec![
            &setup.env,
            String::from_str(&setup.env, "v0"),
            String::from_str(&setup.env, "v1"),
            String::from_str(&setup.env, "v2"),
        ],
        commitments: setup.contract.build_commitments_from_votes(
            &id,
            &vec![&setup.env, 1u128, 0u128, 0u128],
            &vec![&setup.env, 2u128, 0u128, 0u128],
        ),
    });
    setup.contract.vote(&kuiil, &id, &proposal_id, &kuiil_vote);

    // rex votes Reject, weight=3 (Community badge)
    // Without removal: reject=3 ties approve=3 → Cancelled
    // With removal: approve=3 > reject=0 → Approved
    let rex_vote = Vote::AnonymousVote(AnonymousVote {
        address: rex.clone(),
        weight: 3,
        encrypted_seeds: vec![
            &setup.env,
            String::from_str(&setup.env, "r0"),
            String::from_str(&setup.env, "r1"),
            String::from_str(&setup.env, "r2"),
        ],
        encrypted_votes: vec![
            &setup.env,
            String::from_str(&setup.env, "rv0"),
            String::from_str(&setup.env, "rv1"),
            String::from_str(&setup.env, "rv2"),
        ],
        commitments: setup.contract.build_commitments_from_votes(
            &id,
            &vec![&setup.env, 0u128, 1u128, 0u128],
            &vec![&setup.env, 0u128, 3u128, 0u128],
        ),
    });
    setup.contract.vote(&rex, &id, &proposal_id, &rex_vote);

    // Remove rex's reject vote (maintainer action)
    setup
        .contract
        .remove_vote(&setup.grogu, &id, &proposal_id, &rex);

    setup
        .env
        .ledger()
        .set_timestamp(voting_ends_at + TIMELOCK_DELAY + 1);

    let result = setup.contract.execute(
        &setup.grogu,
        &id,
        &proposal_id,
        &Some(vec![&setup.env, 3u128, 0u128, 0u128]),
        &Some(vec![&setup.env, 6u128, 0u128, 0u128]),
    );
    assert_eq!(result, ProposalStatus::Approved);
}

// --- Mutation-testing-driven regression tests -------------------------
//
// The tests below were added after running `cargo mutants` against
// contract_dao.rs (see contracts/tansu/MUTATION_TESTING.md). Each one
// targets a specific mutant that survived the pre-existing suite: every
// other test still passed even with the mutation applied, meaning that
// code path had line/branch coverage but no test actually pinned its
// *behavior*. Comments below reference the exact mutation that motivated
// the test.

#[test]
fn tallies_to_result_boundary_conditions() {
    // Direct unit tests of the supermajority decision function itself
    // (approve > reject+abstain / reject > approve+abstain / else Cancelled).
    // Mutation testing found that every existing end-to-end test used vote
    // weights so lopsided that flipping `>` to `>=`/`==`/`<`, or `+` to
    // `-`/`*`, in tallies_to_result never changed the final ProposalStatus.
    // These cases pin the exact boundary behavior, including ties.
    let env = Env::default();

    // Exact tie (approve == reject + abstain) -> Cancelled, not Approved.
    // Kills `>` -> `>=` and `+` -> `-` on the approve-side comparison.
    assert_eq!(
        contract_dao::public_execute(&vec![&env, 5u128, 3u128, 2u128]),
        ProposalStatus::Cancelled
    );

    // Approve wins by the smallest possible margin.
    assert_eq!(
        contract_dao::public_execute(&vec![&env, 6u128, 3u128, 2u128]),
        ProposalStatus::Approved
    );

    // Reject wins by the smallest possible margin -> Rejected.
    // Kills `>` -> `==` and `>` -> `<` on the reject-side comparison.
    assert_eq!(
        contract_dao::public_execute(&vec![&env, 3u128, 6u128, 2u128]),
        ProposalStatus::Rejected
    );

    // Exact tie on the reject side (reject == approve + abstain) -> Cancelled,
    // not Rejected. Kills `>` -> `>=` and `+` -> `-` on the reject-side check.
    assert_eq!(
        contract_dao::public_execute(&vec![&env, 2u128, 5u128, 3u128]),
        ProposalStatus::Cancelled
    );

    // Reject-side sum where `+` -> `*` would produce a smaller bound
    // (2*4=8 < 2+4=6 is false, so pick values where the product genuinely
    // diverges from the sum): approve=2, abstain=4 -> sum=6, product=8.
    // reject=7 is > the correct sum (Rejected) but not > the mutated product.
    // Kills `+` -> `*` on the reject-side comparison.
    assert_eq!(
        contract_dao::public_execute(&vec![&env, 2u128, 7u128, 4u128]),
        ProposalStatus::Rejected
    );

    // No votes at all -> Cancelled (sanity/edge case).
    assert_eq!(
        contract_dao::public_execute(&vec![&env, 0u128, 0u128, 0u128]),
        ProposalStatus::Cancelled
    );
}

#[test]
fn create_proposal_voting_period_boundary() {
    // Mutation testing found `MIN_VOTING_PERIOD = 24 * 3600` survived being
    // mutated to `24 + 3600` or `24 / 3600`: no test's voting_ends_at was
    // close enough to the true 1-day minimum to notice a shrunk bound.
    let setup = create_test_data();
    let id = init_contract(&setup);

    let title = String::from_str(&setup.env, "Voting period boundary test");
    let ipfs = String::from_str(
        &setup.env,
        "bafybeib6ioupho3p3pliusx7tgs7dvi6mpu2bwfhayj6w6ie44lo3vvc4i",
    );
    let now = setup.env.ledger().timestamp();
    const MIN_VOTING_PERIOD: u64 = 24 * 3600;

    // One second short of the true minimum must be rejected.
    let err = setup
        .contract
        .try_create_proposal(
            &setup.grogu,
            &id,
            &title,
            &ipfs,
            &(now + MIN_VOTING_PERIOD - 1),
            &true,
            &None,
            &None,
        )
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::ProposalInputValidation.into());

    // Exactly the true minimum must be accepted.
    setup.contract.create_proposal(
        &setup.grogu,
        &id,
        &title,
        &ipfs,
        &(now + MIN_VOTING_PERIOD),
        &true,
        &None,
        &None,
    );
}

#[test]
fn create_proposal_input_validation_rejects_each_field_independently() {
    // Mutation testing found the `&&` operators chaining the voting_ends_at /
    // title_len / ipfs_len checks in create_proposal survived being mutated
    // to `||`: every existing negative test broke more than one field at
    // once, so a chain that only rejects when *all* fields are invalid
    // together (instead of when *any* one is) still passed. These cases
    // break exactly one field at a time.
    let setup = create_test_data();
    let id = init_contract(&setup);

    let valid_title = String::from_str(&setup.env, "A perfectly valid title");
    let valid_ipfs = String::from_str(
        &setup.env,
        "bafybeib6ioupho3p3pliusx7tgs7dvi6mpu2bwfhayj6w6ie44lo3vvc4i",
    );
    let valid_voting_ends_at = setup.env.ledger().timestamp() + 3600 * 24 * 2;

    // Title too short (< 10 chars); ipfs and voting_ends_at valid.
    let short_title = String::from_str(&setup.env, "short");
    let err = setup
        .contract
        .try_create_proposal(
            &setup.grogu,
            &id,
            &short_title,
            &valid_ipfs,
            &valid_voting_ends_at,
            &true,
            &None,
            &None,
        )
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::ProposalInputValidation.into());

    // ipfs too short (< 32 chars); title and voting_ends_at valid.
    let short_ipfs = String::from_str(&setup.env, "tooshort");
    let err = setup
        .contract
        .try_create_proposal(
            &setup.grogu,
            &id,
            &valid_title,
            &short_ipfs,
            &valid_voting_ends_at,
            &true,
            &None,
            &None,
        )
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::ProposalInputValidation.into());

    // voting_ends_at too soon; title and ipfs valid.
    let err = setup
        .contract
        .try_create_proposal(
            &setup.grogu,
            &id,
            &valid_title,
            &valid_ipfs,
            &(setup.env.ledger().timestamp() + 10),
            &true,
            &None,
            &None,
        )
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::ProposalInputValidation.into());

    // All three fields valid -> succeeds.
    setup.contract.create_proposal(
        &setup.grogu,
        &id,
        &valid_title,
        &valid_ipfs,
        &valid_voting_ends_at,
        &true,
        &None,
        &None,
    );
}

#[test]
fn proposal_addressing_spans_multiple_pages() {
    // Mutation testing found the `page = proposal_id / MAX_PROPOSALS_PER_PAGE`
    // / `sub_id = proposal_id % MAX_PROPOSALS_PER_PAGE` arithmetic in
    // remove_vote, revoke_proposal, and get_proposal survived `/`<->`%`<->`*`
    // mutations. Getting a real test suite to catch all of them took two
    // rounds:
    //
    //   1. Every pre-existing test only ever addressed proposal_id 0, where
    //      `0/9`, `0%9`, and `0*9` all coincidentally equal 0 -- fixed by
    //      spilling onto page 1 (MAX_PROPOSALS_PER_PAGE is 9).
    //   2. remove_vote's page/sub_id are used *only* to fetch a proposal for
    //      an incidental `status == Active` pre-check; the actual vote
    //      removal keys directly off the original `proposal_id`. So a
    //      misrouted page/sub_id is only observable if the *wrongly*
    //      addressed proposal is not Active (otherwise the pre-check
    //      silently passes and the real removal still succeeds correctly).
    //      Proposals 0 and 10 are revoked ahead of time as decoys so a
    //      misroute lands on a non-Active proposal and panics.
    //
    //   Also: id 10 is a degenerate case where `10/9 == 10%9 == 1`, so it
    //   can't distinguish `/` from `%` for revoke_proposal/get_proposal --
    //   id 9 and id 11 are used for those checks instead. (It's still a
    //   useful decoy for the remove_vote trap above, precisely because
    //   `/`<->`%` is a no-op for it.)
    let setup = create_test_data();
    let id = init_contract(&setup);

    let ipfs = String::from_str(
        &setup.env,
        "bafybeib6ioupho3p3pliusx7tgs7dvi6mpu2bwfhayj6w6ie44lo3vvc4i",
    );
    let voting_ends_at = setup.env.ledger().timestamp() + 3600 * 24 * 2;

    // Create 12 proposals (ids 0..=11): id 9 -> page 1 / sub_id 0,
    // id 10 -> page 1 / sub_id 1, id 11 -> page 1 / sub_id 2.
    for i in 0..12u32 {
        let title = String::from_str(&setup.env, "Multi-page proposal");
        let created_id = setup.contract.create_proposal(
            &setup.grogu,
            &id,
            &title,
            &ipfs,
            &voting_ends_at,
            &true,
            &None,
            &None,
        );
        assert_eq!(created_id, i);
    }

    // get_proposal must resolve page-1 ids to their own data, not to page
    // 0's proposal or to a page-1 neighbor.
    let proposal9 = setup.contract.get_proposal(&id, &9);
    assert_eq!(proposal9.id, 9);
    let proposal11 = setup.contract.get_proposal(&id, &11);
    assert_eq!(proposal11.id, 11);

    // Decoys for the remove_vote trap described above.
    setup.contract.revoke_proposal(&setup.mando, &id, &0);
    setup.contract.revoke_proposal(&setup.mando, &id, &10);

    // remove_vote must operate on proposal 9 specifically.
    setup.contract.vote(
        &setup.mando,
        &id,
        &9,
        &Vote::PublicVote(PublicVote {
            address: setup.mando.clone(),
            weight: 1,
            vote_choice: VoteChoice::Reject,
        }),
    );
    setup.contract.remove_vote(&setup.grogu, &id, &9, &setup.mando);

    let proposal9_after = setup.contract.get_proposal(&id, &9);
    assert_eq!(proposal9_after.status, ProposalStatus::Active);
    let mando_still_present = proposal9_after.vote_data.votes.iter().any(|v| match v {
        Vote::PublicVote(pv) => pv.address == setup.mando,
        Vote::AnonymousVote(av) => av.address == setup.mando,
    });
    assert!(!mando_still_present);

    // revoke_proposal must operate on proposal 11 specifically. A misroute
    // here also lands on the already-revoked decoy (id 10), so it panics
    // instead of silently revoking the wrong proposal.
    setup.contract.revoke_proposal(&setup.mando, &id, &11);
    let proposal11_after = setup.contract.get_proposal(&id, &11);
    assert_eq!(proposal11_after.status, ProposalStatus::Malicious);

    // Sanity: the decoys ended up exactly as intended, nothing more.
    assert_eq!(
        setup.contract.get_proposal(&id, &0).status,
        ProposalStatus::Malicious
    );
    assert_eq!(
        setup.contract.get_proposal(&id, &10).status,
        ProposalStatus::Malicious
    );
}

#[test]
fn remove_vote_removes_exact_voter_from_voters_list() {
    // Mutation testing found the `voters.iter().position(|v| v == voter)`
    // predicate in remove_vote survived being mutated to `!=`: with a
    // single extra voter, "find the first voter equal to the target" and
    // "find the first voter *not* equal to the target" happened to remove
    // the same (only) entry. With three voters, removing the *last* one
    // exposes the difference: the `!=` mutant removes the *first* voter
    // (the proposer) instead.
    let setup = create_test_data();
    let id = init_contract(&setup);

    let ipfs = String::from_str(
        &setup.env,
        "bafybeib6ioupho3p3pliusx7tgs7dvi6mpu2bwfhayj6w6ie44lo3vvc4i",
    );
    let voting_ends_at = setup.env.ledger().timestamp() + 3600 * 24 * 2;

    let proposal_id = setup.contract.create_proposal(
        &setup.grogu,
        &id,
        &String::from_str(&setup.env, "Voter list precision"),
        &ipfs,
        &voting_ends_at,
        &true,
        &None,
        &None,
    );
    // voters is now [grogu (auto-abstain)].

    let rex = Address::generate(&setup.env);
    setup.token_stellar.mint(&rex, &(10 * 10_000_000));
    setup
        .contract
        .add_member(&rex, &String::from_str(&setup.env, "rex"));

    setup.contract.vote(
        &setup.mando,
        &id,
        &proposal_id,
        &Vote::PublicVote(PublicVote {
            address: setup.mando.clone(),
            weight: 1,
            vote_choice: VoteChoice::Approve,
        }),
    );
    setup.contract.vote(
        &rex,
        &id,
        &proposal_id,
        &Vote::PublicVote(PublicVote {
            address: rex.clone(),
            weight: 1,
            vote_choice: VoteChoice::Reject,
        }),
    );
    // voters is now [grogu, mando, rex]; remove the *last* entry (rex).

    setup
        .contract
        .remove_vote(&setup.grogu, &id, &proposal_id, &rex);

    let proposal = setup.contract.get_proposal(&id, &proposal_id);
    assert_eq!(proposal.vote_data.votes.len(), 2);

    let has = |addr: &Address| {
        proposal.vote_data.votes.iter().any(|v| match v {
            Vote::PublicVote(pv) => &pv.address == addr,
            Vote::AnonymousVote(av) => &av.address == addr,
        })
    };
    assert!(has(&setup.grogu), "grogu's vote must remain");
    assert!(has(&setup.mando), "mando's vote must remain");
    assert!(!has(&rex), "rex's vote must be gone");
}

#[test]
fn vote_anonymous_commitment_and_encrypted_field_lengths_are_checked_independently() {
    // Mutation testing found the
    // `encrypted_votes.len() != 3 || encrypted_seeds.len() != 3` check in
    // vote() survived being mutated to `&&`: the existing negative test
    // broke both fields' lengths at once, so a check that only rejects
    // when *both* are wrong (instead of *either*) still passed.
    let setup = create_test_data();
    let id = init_contract(&setup);

    let public_key = String::from_str(&setup.env, "public key random");
    setup
        .contract
        .anonymous_voting_setup(&setup.mando, &id, &public_key);

    let kuiil = Address::generate(&setup.env);
    setup.token_stellar.mint(&kuiil, &(10 * 10_000_000));
    setup
        .contract
        .add_member(&kuiil, &String::from_str(&setup.env, "kuiil"));

    let ipfs = String::from_str(
        &setup.env,
        "bafybeib6ioupho3p3pliusx7tgs7dvi6mpu2bwfhayj6w6ie44lo3vvc4i",
    );
    let voting_ends_at = setup.env.ledger().timestamp() + 3600 * 24 * 2;
    let proposal_id = setup.contract.create_proposal(
        &setup.grogu,
        &id,
        &String::from_str(&setup.env, "Independent length checks"),
        &ipfs,
        &voting_ends_at,
        &false,
        &None,
        &None,
    );

    let commitments = setup.contract.build_commitments_from_votes(
        &id,
        &vec![&setup.env, 1u128, 0u128, 0u128],
        &vec![&setup.env, 1u128, 0u128, 0u128],
    );
    assert_eq!(commitments.len(), 3);

    // Valid commitments and encrypted_seeds; encrypted_votes has the wrong
    // length.
    let err = setup
        .contract
        .try_vote(
            &kuiil,
            &id,
            &proposal_id,
            &Vote::AnonymousVote(AnonymousVote {
                address: kuiil.clone(),
                weight: 1,
                encrypted_seeds: vec![
                    &setup.env,
                    String::from_str(&setup.env, "s0"),
                    String::from_str(&setup.env, "s1"),
                    String::from_str(&setup.env, "s2"),
                ],
                encrypted_votes: vec![
                    &setup.env,
                    String::from_str(&setup.env, "v0"),
                    String::from_str(&setup.env, "v1"),
                ],
                commitments: commitments.clone(),
            }),
        )
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::BadCommitment.into());

    // The converse: valid encrypted_votes; encrypted_seeds has the wrong
    // length.
    let err = setup
        .contract
        .try_vote(
            &kuiil,
            &id,
            &proposal_id,
            &Vote::AnonymousVote(AnonymousVote {
                address: kuiil.clone(),
                weight: 1,
                encrypted_seeds: vec![&setup.env, String::from_str(&setup.env, "s0")],
                encrypted_votes: vec![
                    &setup.env,
                    String::from_str(&setup.env, "v0"),
                    String::from_str(&setup.env, "v1"),
                    String::from_str(&setup.env, "v2"),
                ],
                commitments,
            }),
        )
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::BadCommitment.into());
}

#[test]
fn execute_tallies_and_seeds_length_checks_are_independent() {
    // Same class of gap as the vote() commitment-length test above, but for
    // execute()'s `tallies_.len() != 3 || seeds_.len() != 3` check.
    let setup = create_test_data();
    let id = init_contract(&setup);

    let public_key = String::from_str(&setup.env, "public key random");
    setup
        .contract
        .anonymous_voting_setup(&setup.mando, &id, &public_key);

    let ipfs = String::from_str(
        &setup.env,
        "bafybeib6ioupho3p3pliusx7tgs7dvi6mpu2bwfhayj6w6ie44lo3vvc4i",
    );
    let voting_ends_at = setup.env.ledger().timestamp() + 3600 * 24 * 2;
    let proposal_id = setup.contract.create_proposal(
        &setup.grogu,
        &id,
        &String::from_str(&setup.env, "Length independence"),
        &ipfs,
        &voting_ends_at,
        &false,
        &None,
        &None,
    );

    setup
        .env
        .ledger()
        .set_timestamp(voting_ends_at + TIMELOCK_DELAY + 1);

    // tallies has the wrong length; seeds is a valid length.
    let err = setup
        .contract
        .try_execute(
            &setup.grogu,
            &id,
            &proposal_id,
            &Some(vec![&setup.env, 0u128, 0u128]),
            &Some(vec![&setup.env, 0u128, 0u128, 0u128]),
        )
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::TallySeedError.into());

    // The converse: tallies is a valid length; seeds has the wrong length.
    let err = setup
        .contract
        .try_execute(
            &setup.grogu,
            &id,
            &proposal_id,
            &Some(vec![&setup.env, 0u128, 0u128, 0u128]),
            &Some(vec![&setup.env, 0u128, 0u128]),
        )
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::TallySeedError.into());
}

#[test]
fn execute_respects_exact_timelock_boundary() {
    // Mutation testing found
    // `curr_timestamp < voting_ends_at + TIMELOCK_DELAY` survived being
    // mutated to `<=` (blocking execution at the exact allowed instant) and
    // to `voting_ends_at - TIMELOCK_DELAY` (allowing execution far too
    // early): every existing test executed comfortably past the boundary.
    let setup = create_test_data();
    let id = init_contract(&setup);

    setup.env.ledger().set_timestamp(1_000_000);
    let voting_ends_at = 1_000_000 + 3600 * 24 * 2;

    let proposal_id = setup.contract.create_proposal(
        &setup.grogu,
        &id,
        &String::from_str(&setup.env, "Timelock boundary"),
        &String::from_str(
            &setup.env,
            "bafybeib6ioupho3p3pliusx7tgs7dvi6mpu2bwfhayj6w6ie44lo3vvc4i",
        ),
        &voting_ends_at,
        &true,
        &None,
        &None,
    );

    // One second before the boundary: must still be rejected.
    setup
        .env
        .ledger()
        .set_timestamp(voting_ends_at + TIMELOCK_DELAY - 1);
    let err = setup
        .contract
        .try_execute(&setup.mando, &id, &proposal_id, &None, &None)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractErrors::ProposalVotingTime.into());

    // Exactly at the boundary: must be allowed.
    setup
        .env
        .ledger()
        .set_timestamp(voting_ends_at + TIMELOCK_DELAY);
    let result = setup
        .contract
        .execute(&setup.mando, &id, &proposal_id, &None, &None);
    assert_eq!(result, ProposalStatus::Cancelled); // no votes beyond proposer's auto-abstain
}

#[test]
fn outcomes_execution_rejected() {
    // Mutation testing found the outcome_contracts match arm for
    // `ProposalStatus::Rejected` (outcome_index 1) survived being deleted
    // entirely: the only existing outcome-contract test drove proposals to
    // Approved, so a missing Rejected arm (which falls through to the
    // catch-all `panic_with_error!(OutcomeError)`) was never exercised.
    let setup = create_test_data();
    let id = init_contract(&setup);

    let outcome_contract_id = setup.env.register(TestOutcomeContract, ());

    setup.env.ledger().set_timestamp(1234567890);
    let voting_ends_at = 1234567890 + 3600 * 24 * 2;
    let ipfs = String::from_str(
        &setup.env,
        "bafybeib6ioupho3p3pliusx7tgs7dvi6mpu2bwfhayj6w6ie44lo3vvc4i",
    );

    // Only the Rejected (index 1) arm is exercised; indices 0 and 2 point at
    // non-contract addresses and must never be invoked on this path.
    let outcome_contracts = vec![
        &setup.env,
        OutcomeContract {
            address: Address::generate(&setup.env),
            execute_fn: Symbol::new(&setup.env, "unused_approve"),
            args: vec![&setup.env],
        },
        OutcomeContract {
            address: outcome_contract_id.clone(),
            execute_fn: Symbol::new(&setup.env, "execute_reject"),
            args: vec![
                &setup.env,
                setup.mando.clone().into_val(&setup.env),
                200u32.into_val(&setup.env),
            ],
        },
        OutcomeContract {
            address: Address::generate(&setup.env),
            execute_fn: Symbol::new(&setup.env, "unused_cancel"),
            args: vec![&setup.env],
        },
    ];

    let proposal_id = setup.contract.create_proposal(
        &setup.grogu,
        &id,
        &String::from_str(&setup.env, "Reject this proposal"),
        &ipfs,
        &voting_ends_at,
        &true,
        &None,
        &Some(outcome_contracts),
    );

    setup.contract.vote(
        &setup.mando,
        &id,
        &proposal_id,
        &Vote::PublicVote(PublicVote {
            address: setup.mando.clone(),
            weight: 1,
            vote_choice: VoteChoice::Reject,
        }),
    );

    setup
        .env
        .ledger()
        .set_timestamp(voting_ends_at + TIMELOCK_DELAY + 1);
    let result = setup
        .contract
        .execute(&setup.mando, &id, &proposal_id, &None, &None);
    assert_eq!(result, ProposalStatus::Rejected);

    let proposal = setup.contract.get_proposal(&id, &proposal_id);
    assert_eq!(proposal.status, ProposalStatus::Rejected);
}

#[test]
fn outcomes_execution_cancelled() {
    // Same class of gap as outcomes_execution_rejected, but for the
    // `ProposalStatus::Cancelled` (outcome_index 2) match arm.
    let setup = create_test_data();
    let id = init_contract(&setup);

    let outcome_contract_id = setup.env.register(TestOutcomeContract, ());

    setup.env.ledger().set_timestamp(1234567890);
    let voting_ends_at = 1234567890 + 3600 * 24 * 2;
    let ipfs = String::from_str(
        &setup.env,
        "bafybeib6ioupho3p3pliusx7tgs7dvi6mpu2bwfhayj6w6ie44lo3vvc4i",
    );

    // Only the Cancelled (index 2) arm is exercised; indices 0 and 1 point
    // at non-contract addresses and must never be invoked on this path.
    let outcome_contracts = vec![
        &setup.env,
        OutcomeContract {
            address: Address::generate(&setup.env),
            execute_fn: Symbol::new(&setup.env, "unused_approve"),
            args: vec![&setup.env],
        },
        OutcomeContract {
            address: Address::generate(&setup.env),
            execute_fn: Symbol::new(&setup.env, "unused_reject"),
            args: vec![&setup.env],
        },
        OutcomeContract {
            address: outcome_contract_id.clone(),
            execute_fn: Symbol::new(&setup.env, "execute_cancel"),
            args: vec![
                &setup.env,
                setup.mando.clone().into_val(&setup.env),
                0u32.into_val(&setup.env),
            ],
        },
    ];

    let proposal_id = setup.contract.create_proposal(
        &setup.grogu,
        &id,
        &String::from_str(&setup.env, "Cancel this proposal"),
        &ipfs,
        &voting_ends_at,
        &true,
        &None,
        &Some(outcome_contracts),
    );
    // No further votes: proposer's auto-abstain (weight 0) alone yields
    // tallies [0,0,0], a tie -> Cancelled.

    setup
        .env
        .ledger()
        .set_timestamp(voting_ends_at + TIMELOCK_DELAY + 1);
    let result = setup
        .contract
        .execute(&setup.mando, &id, &proposal_id, &None, &None);
    assert_eq!(result, ProposalStatus::Cancelled);

    let proposal = setup.contract.get_proposal(&id, &proposal_id);
    assert_eq!(proposal.status, ProposalStatus::Cancelled);
}
