//! Test helpers, mock environments, and assertion utilities.
//! Gated behind the `testutils` feature flag (Issue #709).

use soroban_sdk::{
    testutils::{Address as _, Ledger as _, MockAuth, MockAuthInvoke},
    Address, Env,
};

/// Generates a test address for unit and integration testing.
pub fn create_test_address(env: &Env) -> Address {
    Address::generate(env)
}

/// Helper to configure a test environment with all authorizations mocked.
pub fn setup_test_env() -> Env {
    let env = Env::default();
    env.mock_all_auths();
    env
}

/// Helper to advance ledger sequence and timestamp in tests.
pub fn advance_ledger(env: &Env, ledgers: u32, seconds: u64) {
    env.ledger().with_mut(|li| {
        li.sequence_number += ledgers;
        li.timestamp += seconds;
    });
}

/// Advances both the ledger clock and the ledger sequence in one call.
///
/// Test fixtures previously reimplemented this pair of updates in every
/// contract's test module; keep the argument order explicit so call sites read
/// the same everywhere.
pub fn advance_time_and_ledgers(env: &Env, seconds: u64, ledgers: u32) {
    env.ledger().with_mut(|li| {
        li.timestamp += seconds;
        li.sequence_number += ledgers;
    });
}

/// Registers a Stellar asset contract with `admin` as issuer and returns its
/// address, ready to mint test balances.
pub fn create_mock_token(env: &Env, admin: &Address) -> Address {
    env.register_stellar_asset_contract_v2(admin.clone())
        .address()
}

/// Mocks authorization for `address` for exactly one invocation context.
///
/// `setup_test_env` authorizes everything with `mock_all_auths`, which hides
/// missing or wrong auth requirements. Use this when a test needs to assert
/// that a specific `address` authorized a specific `invocation` (contract,
/// function name, arguments and any sub-invocations) and nothing else: any call
/// that does not match fails its authorization check.
///
/// `invocation` is the SDK's `MockAuthInvoke`; build `args` with
/// `(&a, b).into_val(env)`.
pub fn mock_auth_context(env: &Env, address: &Address, invocation: &MockAuthInvoke) {
    env.mock_auths(&[MockAuth {
        address,
        invoke: invocation,
    }]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{token, IntoVal};

    #[test]
    fn advance_time_and_ledgers_updates_both_clock_and_sequence() {
        let env = setup_test_env();
        let before = env.ledger().get();

        advance_time_and_ledgers(&env, 120, 24);
        let after = env.ledger().get();

        assert_eq!(after.timestamp, before.timestamp + 120);
        assert_eq!(after.sequence_number, before.sequence_number + 24);
    }

    #[test]
    fn mock_auth_context_authorizes_only_the_given_invocation() {
        let env = Env::default();
        let admin = create_test_address(&env);
        let holder = create_test_address(&env);
        let token_addr = create_mock_token(&env, &admin);
        let asset = token::StellarAssetClient::new(&env, &token_addr);

        mock_auth_context(
            &env,
            &admin,
            &MockAuthInvoke {
                contract: &token_addr,
                fn_name: "mint",
                args: (&holder, 100i128).into_val(&env),
                sub_invokes: &[],
            },
        );
        asset.mint(&holder, &100);
        assert_eq!(token::Client::new(&env, &token_addr).balance(&holder), 100);

        // A different invocation was never mocked, so it must not be authorized.
        assert!(asset.try_mint(&holder, &50).is_err());
    }

    #[test]
    fn create_mock_token_returns_a_working_asset_contract() {
        let env = setup_test_env();
        let admin = create_test_address(&env);
        let holder = create_test_address(&env);
        let token_addr = create_mock_token(&env, &admin);

        token::StellarAssetClient::new(&env, &token_addr).mint(&holder, &500);
        assert_eq!(token::Client::new(&env, &token_addr).balance(&holder), 500);
    }
}
