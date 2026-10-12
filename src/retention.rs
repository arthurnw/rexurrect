//! Retention: how many snapshots rex-resurrect keeps.
//!
//! [`Retention::prune`] works on any directory whose entries are named so
//! that they sort in time order ([`crate::timestamp::Timestamp::compact`]),
//! files or directories alike, so whatever belongs to a snapshot can be kept
//! on the same terms as the snapshot itself.

use std::ffi::{OsStr, OsString};
use std::num::NonZeroUsize;
use std::path::Path;

use crate::error::{Error, Result};
use crate::settings;
use crate::state;

pub const KEEP_VAR: &str = "REX_RESURRECT_KEEP";
const DEFAULT_KEEP: usize = 10;

/// How many snapshots to keep: the newest N.
#[derive(Debug, Clone, Copy)]
pub struct Retention {
    keep: NonZeroUsize,
}

impl Retention {
    /// `$REX_RESURRECT_KEEP` snapshots, 10 if it's unset or empty. Anything
    /// but a whole number of at least 1 is an error.
    pub fn from_env() -> Result<Retention> {
        let Some(value) = settings::env_non_empty(KEEP_VAR) else {
            return Ok(Retention {
                keep: NonZeroUsize::new(DEFAULT_KEEP).expect("the default is at least 1"),
            });
        };
        let keep = value
            .to_str()
            .and_then(settings::whole_number_at_least_1)
            .and_then(|keep| NonZeroUsize::try_from(keep).ok())
            .ok_or_else(|| {
                Error::new(format!(
                    "{KEEP_VAR} must be a whole number of snapshots to keep, at least 1, not {value:?}"
                ))
            })?;
        Ok(Retention { keep })
    }

    /// Removes all but the newest N entries of `dir` that `belongs` picks,
    /// newest by name. `always_keep` (the entry "latest" points at) is never
    /// removed and counts as one of the N, even if its name sorts older (a
    /// clock that went backwards). Other entries are left alone.
    pub fn prune(
        &self,
        dir: &Path,
        always_keep: &OsStr,
        belongs: impl Fn(&OsStr) -> bool,
    ) -> Result<()> {
        let mut names: Vec<OsString> = state::read_dir_or_empty(dir)?
            .iter()
            .map(|entry| entry.file_name())
            .collect();
        let room = self.keep.get() - usize::from(names.iter().any(|n| n == always_keep));
        names.retain(|name| name != always_keep && belongs(name));
        names.sort_unstable_by(|a, b| b.cmp(a));
        for name in names.iter().skip(room) {
            state::remove_entry(&dir.join(name))?;
        }
        Ok(())
    }
}
