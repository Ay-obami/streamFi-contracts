//! Emergency-pause state for `DripFactory`.
//!
//! Two layers, read together through [`effective_flags`]:
//!
//! - the **global level** ([`PauseType`], Issue #767), set by `pause`,
//!   `unpause` and `pause_with_level`. Its footprint is fixed by
//!   [`global_flags`];
//! - the **granular flags** ([`PauseFlags`], Issue #975), an operator overlay
//!   stored under `DataKey::PauseFlags` and written by `set_pause_flags`.
//!
//! The layers are OR-ed. A global pause keeps its footprint whatever the
//! overlay holds, and clearing the overlay cannot lift it. Clearing the global
//! level leaves any explicitly set granular flags in force.
//!
//! No storage migration is needed. A factory that predates this change has no
//! `DataKey::PauseFlags` key and reads as zero granular flags, and its existing
//! `PauseLevel` / `Paused` keys keep their meaning.

use drip_common::pause::PauseFlags;
use soroban_sdk::{contracttype, Env};

use crate::errors::Error;
use crate::storage::DataKey;

/// Granular pause levels for creation vs withdrawal and emergency operations (Issue #767).
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum PauseType {
    Unpaused = 0,
    CreationOnly = 1,
    FullEmergency = 2,
}

/// Reads the current granular pause level.
pub fn pause_level(env: &Env) -> PauseType {
    if let Some(level) = env.storage().instance().get(&DataKey::PauseLevel) {
        level
    } else if drip_common::pause::is_paused(env, &DataKey::Paused) {
        PauseType::FullEmergency
    } else {
        PauseType::Unpaused
    }
}

/// Sets the granular pause level and updates the boolean flag in instance storage.
pub fn set_pause_level(env: &Env, level: PauseType) {
    env.storage().instance().set(&DataKey::PauseLevel, &level);
    let is_paused = level != PauseType::Unpaused;
    let _ = drip_common::pause::set_paused(env, &DataKey::Paused, is_paused);
}

/// Reads the emergency-pause flag.
///
/// Defaults to `false` (unpaused) when the key has never been set. Reports the
/// global level only: granular flags do not flip it.
pub fn is_paused(env: &Env) -> bool {
    pause_level(env) != PauseType::Unpaused
}

/// Writes the emergency-pause flag directly to instance storage.
pub fn set_paused(env: &Env, paused: bool) {
    let level = if paused {
        PauseType::FullEmergency
    } else {
        PauseType::Unpaused
    };
    set_pause_level(env, level);
}

/// The flags a global level imposes.
///
/// - `Unpaused`: none.
/// - `CreationOnly`: creations and deposits, as before. Withdrawals stay open.
/// - `FullEmergency`: every flag, the footprint the level has always carried.
pub fn global_flags(level: PauseType) -> PauseFlags {
    match level {
        PauseType::Unpaused => PauseFlags::empty(),
        PauseType::CreationOnly => PauseFlags::PAUSE_DEPOSITS.union(PauseFlags::PAUSE_CREATIONS),
        PauseType::FullEmergency => PauseFlags::ALL,
    }
}

/// The granular overlay stored under `DataKey::PauseFlags`. Missing or
/// unrecognised bits read as empty.
pub fn granular_flags(env: &Env) -> PauseFlags {
    let bits: u32 = env
        .storage()
        .instance()
        .get(&DataKey::PauseFlags)
        .unwrap_or(0);
    PauseFlags::from_bits(bits & PauseFlags::ALL.bits()).unwrap_or(PauseFlags::empty())
}

/// Writes the granular overlay. Leaves the global level untouched.
pub fn set_granular_flags(env: &Env, flags: PauseFlags) {
    env.storage()
        .instance()
        .set(&DataKey::PauseFlags, &flags.bits());
}

/// Everything currently paused: the global level's footprint plus the overlay.
pub fn effective_flags(env: &Env) -> PauseFlags {
    global_flags(pause_level(env)).union(granular_flags(env))
}

/// Rejects new stream creation while `PAUSE_DEPOSITS` or `PAUSE_CREATIONS` is in effect.
///
/// The factory has no deposit-free creation path, so both flags block the
/// same entry points today. `PAUSE_DEPOSITS` is the one that will also cover
/// top-ups once the stream contract reads these flags.
pub fn require_creation_allowed(env: &Env) -> Result<(), Error> {
    let blocked = PauseFlags::PAUSE_DEPOSITS.union(PauseFlags::PAUSE_CREATIONS);
    if effective_flags(env).intersects(blocked) {
        Err(Error::ContractPaused)
    } else {
        Ok(())
    }
}

/// The global pause gate used by the governor-only upgrade path: `Err(ContractPaused)`
/// while the global level is anything but `Unpaused`. Granular flags do not
/// block it, which keeps a partial pause from freezing upgrades.
pub fn require_not_paused(env: &Env) -> Result<(), Error> {
    match pause_level(env) {
        PauseType::Unpaused => Ok(()),
        _ => Err(Error::ContractPaused),
    }
}

/// Rejects while `PAUSE_WITHDRAWALS` is in effect.
///
/// Not called from inside this crate yet. Withdrawals execute on the stream
/// contract, which does not read factory state, so the flag is advisory until
/// that cross-contract read is wired. See `NOTES.md`.
#[allow(dead_code)]
pub fn require_withdrawals_allowed(env: &Env) -> Result<(), Error> {
    if effective_flags(env).intersects(PauseFlags::PAUSE_WITHDRAWALS) {
        Err(Error::ContractPaused)
    } else {
        Ok(())
    }
}
