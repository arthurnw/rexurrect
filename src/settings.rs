//! Reading settings from the environment, and the numbers they hold.

use std::ffi::OsString;
use std::num::NonZeroU64;

/// The environment variable `name`, unless it's unset or empty: an empty
/// variable counts as unset.
pub fn env_non_empty(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

/// A whole number of at least 1 (`"15"`, spaces around it allowed); `None`
/// for anything else (`0`, `-5`, `1.5`, `15m`, nothing).
pub fn whole_number_at_least_1(text: &str) -> Option<NonZeroU64> {
    text.trim().parse().ok()
}
