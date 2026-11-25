//! Integration tests: DripFactory emergency pause.
//!
//! Covers the governor-gated `pause`/`unpause` controls and the guard they
//! place on `create_stream`. These tests exercise the validation/guard paths
//! that do not require an actual stream deployment (which needs the stream
//! WASM to be built first).

use drip_common::pause::PauseFlags;
use drip_factory::{DripFactory, DripFactoryClient, Error, PauseType};
use drip_governor::{DripGovernor, DripGovernorClient};
use soroban_sdk::{
    testutils::{storage::Instance as _, Address as _, Ledger, LedgerInfo},
    token, Address, BytesN, Env,
};

fn base_env() -> Env {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        timestamp: 1_000_000,
        protocol_version: 21,
        sequence_number: 1,
        network_id: Default::default(),
        base_reserve: 10,
        min_temp_entry_ttl: 16,
        min_persistent_entry_ttl: 4096,
        max_entry_ttl: 6_312_000,
    });
    env
}

fn deploy_factory(env: &Env) -> DripFactoryClient<'_> {
    let factory_id = env.register_contract(None, DripFactory);
    let governor_id = env.register_contract(None, DripGovernor);

    let authority = Address::generate(env);
    let fee_recipient = Address::generate(env);
    let governor_client = DripGovernorClient::new(env, &governor_id);
    governor_client.initialize(&authority, &fee_recipient, &factory_id);

    let client = DripFactoryClient::new(env, &factory_id);
    let dummy_hash = BytesN::from_array(env, &[1u8; 32]);
    client.initialize(&dummy_hash, &governor_id);
    client
}

fn make_token(env: &Env, sender: &Address, amount: i128) -> Address {
    let admin = Address::generate(env);
    let addr = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    token::StellarAssetClient::new(env, &addr).mint(sender, &amount);
    addr
}

// ── Default state ──────────────────────────────────────────────────────────────

#[test]
fn factory_starts_unpaused() {
    let env = base_env();
    let client = deploy_factory(&env);
    assert!(!client.is_paused());
}

#[test]
fn factory_status_reflects_pause_state_and_protocol_fee() {
    // A real governor is deployed and initialised here, so 30 bps is the fee
    // it actually holds — `Some(30)`, not the old ambiguous bare `30`.
    let env = base_env();
    let client = deploy_factory(&env);
    let status = client.factory_status();
    assert!(!status.is_paused);
    assert_eq!(status.protocol_fee_bps, Some(30));

    client.pause();
    let status_paused = client.factory_status();
    assert!(status_paused.is_paused);
    assert_eq!(status_paused.protocol_fee_bps, Some(30));
}

#[test]
fn a_real_fee_of_30_is_distinguishable_from_an_unreadable_one() {
    // The point of issue #339, end to end. Both factories would previously have
    // reported a bare `30`; only one of them was telling the truth.
    let env = base_env();

    // Governor deployed and reachable — 30 bps is a real, configured value.
    let with_governor = deploy_factory(&env);
    assert_eq!(with_governor.protocol_fee_bps(), 30);
    assert_eq!(with_governor.factory_status().protocol_fee_bps, Some(30));

    // Governor address points at nothing — the fee cannot be read at all.
    let orphan_id = env.register_contract(None, DripFactory);
    let orphan = DripFactoryClient::new(&env, &orphan_id);
    let dummy_hash = BytesN::from_array(&env, &[1u8; 32]);
    orphan.initialize(&dummy_hash, &Address::generate(&env));

    assert_eq!(orphan.factory_status().protocol_fee_bps, None);
    assert!(orphan.try_protocol_fee_bps().is_err());
}

// ── Pause / unpause lifecycle ──────────────────────────────────────────────────

#[test]
fn governor_can_pause_and_unpause() {
    let env = base_env();
    let client = deploy_factory(&env);

    client.pause();
    assert!(client.is_paused());

    client.unpause();
    assert!(!client.is_paused());
}

#[test]
fn pausing_when_already_paused_is_rejected() {
    let env = base_env();
    let client = deploy_factory(&env);

    client.pause();
    let result = client.try_pause();
    assert_eq!(result, Err(Ok(Error::AlreadyPaused)));
}

#[test]
fn unpausing_when_not_paused_is_rejected() {
    let env = base_env();
    let client = deploy_factory(&env);

    let result = client.try_unpause();
    assert_eq!(result, Err(Ok(Error::NotPaused)));
}

#[test]
fn pause_on_uninitialized_factory_is_rejected() {
    let env = base_env();
    let id = env.register_contract(None, DripFactory);
    let client = DripFactoryClient::new(&env, &id);
    let result = client.try_pause();
    assert_eq!(result, Err(Ok(Error::NotInitialized)));
}

#[test]
fn unpause_on_uninitialized_factory_is_rejected() {
    let env = base_env();
    let id = env.register_contract(None, DripFactory);
    let client = DripFactoryClient::new(&env, &id);
    let result = client.try_unpause();
    assert_eq!(result, Err(Ok(Error::NotInitialized)));
}

// ── create_stream guard ────────────────────────────────────────────────────────

#[test]
fn create_stream_is_rejected_while_paused() {
    let env = base_env();
    let client = deploy_factory(&env);
    client.pause();

    let sender = Address::generate(&env);
    let recip = Address::generate(&env);
    // Fully valid parameters — the only reason this must fail is the pause,
    // which short-circuits before any validation or deposit transfer.
    let token = make_token(&env, &sender, 1_000_000);
    let now = env.ledger().timestamp();
    let result = client.try_create_stream(
        &sender,
        &recip,
        &token,
        &1_000_000,
        &100,
        &now,
        &(now + 7_200),
        &false,
        &None,
        &None,
    );
    assert_eq!(result, Err(Ok(Error::ContractPaused)));
}

#[test]
fn create_stream_works_again_after_unpause() {
    let env = base_env();
    let client = deploy_factory(&env);
    client.pause();
    client.unpause();

    // With a dummy (all-zero) stream WASM hash the deploy step cannot succeed,
    // but the pause guard runs first: once unpaused, create_stream progresses
    // past the guard and fails later for an unrelated reason — never with
    // ContractPaused.
    let sender = Address::generate(&env);
    let recip = Address::generate(&env);
    let token = make_token(&env, &sender, 1_000_000);
    let now = env.ledger().timestamp();
    let result = client.try_create_stream(
        &sender,
        &recip,
        &token,
        &1_000_000,
        &100,
        &now,
        &(now + 7_200),
        &false,
        &None,
        &None,
    );
    assert_ne!(result, Err(Ok(Error::ContractPaused)));
}

#[test]
fn pause_extends_instance_ttl() {
    let env = base_env();
    let client = deploy_factory(&env);
    client.pause();
    let ttl = env.as_contract(&client.address, || env.storage().instance().get_ttl());
    assert_eq!(ttl, 200_000);
}

// ── Event emission & Governor passthrough (Issue #628) ──────────────────────────

#[test]
fn governor_pause_factory_and_unpause_factory_flow() {
    let env = base_env();
    let factory_id = env.register_contract(None, DripFactory);
    let governor_id = env.register_contract(None, DripGovernor);

    let authority = Address::generate(&env);
    let fee_recipient = Address::generate(&env);
    let governor_client = DripGovernorClient::new(&env, &governor_id);
    governor_client.initialize(&authority, &fee_recipient, &factory_id);

    let factory_client = DripFactoryClient::new(&env, &factory_id);
    let dummy_hash = BytesN::from_array(&env, &[1u8; 32]);
    factory_client.initialize(&dummy_hash, &governor_id);

    // Initial state: not paused
    assert!(!factory_client.is_paused());

    // Governor pauses factory
    governor_client.pause_factory(&authority);
    assert!(factory_client.is_paused());

    // Governor unpauses factory
    governor_client.unpause_factory(&authority);
    assert!(!factory_client.is_paused());
}

// ── Granular pause flags (Issue #975) ──────────────────────────────────────────

/// Attempts a fully valid `create_stream`, so the only reason it can be
/// rejected is a pause.
macro_rules! try_create {
    ($env:expr, $client:expr) => {{
        let sender = Address::generate($env);
        let recip = Address::generate($env);
        let token = make_token($env, &sender, 1_000_000);
        let now = $env.ledger().timestamp();
        $client.try_create_stream(
            &sender,
            &recip,
            &token,
            &1_000_000,
            &100,
            &now,
            &(now + 7_200),
            &false,
            &None,
            &None,
        )
    }};
}

#[test]
fn pause_deposits_blocks_creation_but_not_withdrawals() {
    let env = base_env();
    let client = deploy_factory(&env);
    client.set_pause_flags(&PauseFlags::PAUSE_DEPOSITS.bits());

    assert_eq!(client.pause_flags(), PauseFlags::PAUSE_DEPOSITS.bits());
    // A granular flag must not flip the global pause.
    assert!(!client.is_paused());
    assert_eq!(try_create!(&env, &client), Err(Ok(Error::ContractPaused)));
    // Withdrawals stay open under a deposit lock.
    assert_eq!(client.pause_flags() & PauseFlags::PAUSE_WITHDRAWALS.bits(), 0);
}

#[test]
fn pause_withdrawals_blocks_only_the_withdrawal_flag() {
    let env = base_env();
    let client = deploy_factory(&env);
    client.set_pause_flags(&PauseFlags::PAUSE_WITHDRAWALS.bits());

    assert_eq!(client.pause_flags(), PauseFlags::PAUSE_WITHDRAWALS.bits());
    assert_eq!(client.pause_flags() & PauseFlags::PAUSE_DEPOSITS.bits(), 0);
    assert!(!client.is_paused());
    // Creation is unaffected by a withdrawal-only pause.
    assert_ne!(try_create!(&env, &client), Err(Ok(Error::ContractPaused)));
}

#[test]
fn global_pause_still_blocks_creation_and_sets_every_flag() {
    let env = base_env();
    let client = deploy_factory(&env);
    client.pause();

    assert!(client.is_paused());
    assert_eq!(client.pause_flags(), PauseFlags::ALL.bits());
    assert_eq!(try_create!(&env, &client), Err(Ok(Error::ContractPaused)));
}

#[test]
fn clearing_granular_flags_cannot_lift_a_global_pause() {
    let env = base_env();
    let client = deploy_factory(&env);
    client.pause();
    client.set_pause_flags(&0);

    assert!(client.is_paused());
    assert_eq!(client.pause_flags(), PauseFlags::ALL.bits());
    assert_eq!(try_create!(&env, &client), Err(Ok(Error::ContractPaused)));
}

#[test]
fn creation_only_level_blocks_creation_and_keeps_withdrawals_open() {
    let env = base_env();
    let client = deploy_factory(&env);
    client.pause_with_level(&PauseType::CreationOnly);

    let expected = PauseFlags::PAUSE_DEPOSITS.union(PauseFlags::PAUSE_CREATIONS);
    assert_eq!(client.pause_flags(), expected.bits());
    assert_eq!(client.pause_flags() & PauseFlags::PAUSE_WITHDRAWALS.bits(), 0);
    assert!(client.is_paused());
    assert_eq!(try_create!(&env, &client), Err(Ok(Error::ContractPaused)));
}

#[test]
fn unpause_restores_creation_and_clears_global_flags() {
    let env = base_env();
    let client = deploy_factory(&env);
    client.pause();
    client.unpause();

    assert!(!client.is_paused());
    assert_eq!(client.pause_flags(), 0);
    assert_ne!(try_create!(&env, &client), Err(Ok(Error::ContractPaused)));
}

#[test]
fn unpause_keeps_explicitly_set_granular_flags() {
    let env = base_env();
    let client = deploy_factory(&env);
    client.set_pause_flags(&PauseFlags::PAUSE_DEPOSITS.bits());
    client.pause();
    client.unpause();

    assert_eq!(client.pause_flags(), PauseFlags::PAUSE_DEPOSITS.bits());
    assert_eq!(try_create!(&env, &client), Err(Ok(Error::ContractPaused)));
}

#[test]
fn set_pause_flags_rejects_unknown_bits() {
    let env = base_env();
    let client = deploy_factory(&env);

    let result = client.try_set_pause_flags(&(1u32 << 31));
    assert_eq!(result, Err(Ok(Error::InvalidPauseFlags)));
    assert_eq!(client.pause_flags(), 0);
}
