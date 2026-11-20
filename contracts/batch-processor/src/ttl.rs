/// Extend the processor instance's storage TTL.
///
/// Mirrors `contracts/stream/src/ttl.rs`: every state-mutating call renews the
/// instance record while its remaining TTL is at or below `MIN_TTL_LEDGERS`
/// (`TTL_THRESHOLD`), pushing it out to `EXTEND_TTL_LEDGERS` (`TTL_EXTEND_TO`).
///
/// The processor is stateless — it stores no keys — but its *instance* entry
/// still lives on the ledger and can be archived like any other contract's.
/// Without this bump a deployed processor that goes quiet between payouts
/// would eventually be archived (the exposure `docs/security.md` Known
/// Limitation #1 tracks), and every later batch would need a restore first.
use soroban_sdk::{contracttype, token, Address, Env};

pub use drip_common::ttl::bump_instance as bump;

// Automated key and instance TTL extension helpers (Issue #762).
/// Bumps temporary storage key TTL to default thresholds.
pub fn bump_temporary_key<K: soroban_sdk::IntoVal<Env, soroban_sdk::Val>>(env: &Env, key: &K) {
    env.storage().temporary().extend_ttl(
        key,
        drip_common::ttl::MIN_TTL_LEDGERS,
        drip_common::ttl::EXTEND_TTL_LEDGERS,
    );
}

/// Extends active batch execution state and instance storage TTL (Issue #762).
pub fn bump_batch_state(env: &Env) {
    bump(env);
    if env.storage().temporary().has(&crate::DataKey::ReentrancyLock) {
        bump_temporary_key(env, &crate::DataKey::ReentrancyLock);
    }
}

/// Bumps batch processor instance and temporary state TTL to match a target stream's expiration (Issue #762).
pub fn bump_to_match_stream_expiration(env: &Env, target_stream_expiration_ledger: u32) {
    let current_ledger = env.ledger().sequence();
    if target_stream_expiration_ledger > current_ledger {
        let ledgers_remaining = target_stream_expiration_ledger - current_ledger;
        let extend_to = ledgers_remaining.max(drip_common::ttl::EXTEND_TTL_LEDGERS);
        let min_ttl = (extend_to / 2).max(drip_common::ttl::MIN_TTL_LEDGERS);
        env.storage().instance().extend_ttl(min_ttl, extend_to);
        if env.storage().temporary().has(&crate::DataKey::ReentrancyLock) {
            env.storage()
                .temporary()
                .extend_ttl(&crate::DataKey::ReentrancyLock, min_ttl, extend_to);
        }
    } else {
        bump_batch_state(env);
    }
}

/// Automated helper to extend instance and all active temporary keys (Issue #762).
pub fn bump_instance_and_keys(env: &Env) {
    bump_batch_state(env);
}

/// Maintenance reserve pool used to reimburse keepers for extending contract TTL (Issue #817).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaintenanceReserve {
    pub token: Address,
    pub rebate_amount: i128,
    pub balance: i128,
}

pub fn configure_maintenance_reserve(
    env: &Env,
    token: &Address,
    rebate_amount: i128,
) -> Result<MaintenanceReserve, crate::Error> {
    if rebate_amount <= 0 {
        return Err(crate::Error::InvalidAmount);
    }
    let balance = get_maintenance_reserve(env).map(|r| r.balance).unwrap_or(0);
    let reserve = MaintenanceReserve {
        token: token.clone(),
        rebate_amount,
        balance,
    };
    env.storage()
        .instance()
        .set(&crate::DataKey::MaintenanceReserve, &reserve);
    Ok(reserve)
}

pub fn get_maintenance_reserve(env: &Env) -> Option<MaintenanceReserve> {
    env.storage()
        .instance()
        .get(&crate::DataKey::MaintenanceReserve)
}

pub fn fund_maintenance_reserve(
    env: &Env,
    funder: &Address,
    amount: i128,
) -> Result<MaintenanceReserve, crate::Error> {
    if amount <= 0 {
        return Err(crate::Error::InvalidAmount);
    }
    let mut reserve = get_maintenance_reserve(env).ok_or(crate::Error::NotInitialized)?;
    reserve.balance = reserve
        .balance
        .checked_add(amount)
        .ok_or(crate::Error::ArithmeticOverflow)?;

    funder.require_auth();
    token::Client::new(env, &reserve.token).transfer(
        funder,
        &env.current_contract_address(),
        &amount,
    );
    env.storage()
        .instance()
        .set(&crate::DataKey::MaintenanceReserve, &reserve);
    Ok(reserve)
}

/// Extends the processor's storage TTL and reimburses keeper gas rebate from maintenance reserve (Issue #817).
pub fn bump_with_keeper_rebate(env: &Env, keeper: &Address) -> Result<i128, crate::Error> {
    bump(env);

    if let Some(mut reserve) = get_maintenance_reserve(env) {
        if reserve.rebate_amount > 0 && reserve.balance >= reserve.rebate_amount {
            let rebate = reserve.rebate_amount;
            reserve.balance = reserve.balance.saturating_sub(rebate);
            env.storage()
                .instance()
                .set(&crate::DataKey::MaintenanceReserve, &reserve);
            token::Client::new(env, &reserve.token).transfer(
                &env.current_contract_address(),
                keeper,
                &rebate,
            );
            return Ok(rebate);
        }
    }
    Ok(0)
}
