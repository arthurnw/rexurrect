//! A long-running program built by the test itself, for tests that need an
//! executable they can copy and rename (a `claude` installed as a file named
//! for its version, say).
//!
//! A copy of a system binary won't do: some macOS versions kill a copy of
//! `/bin/sleep` run from elsewhere even after it's signed again ad hoc. A
//! program built locally carries the linker's own signature, so it runs from
//! any path and under any name. It's compiled with `rustc` at test time
//! rather than declared as a `[[bin]]` or example, so it never ships with
//! `cargo install` and a single test file run with `--test` still has it.

use std::path::Path;
use std::process::Command;

use super::env::TestEnv;

/// The program's source: sleeps for the number of seconds it's given.
const SOURCE: &str = r#"fn main() {
    let seconds = std::env::args().nth(1).expect("seconds").parse().expect("a number");
    std::thread::sleep(std::time::Duration::from_secs(seconds));
}
"#;

/// Builds the sleeper as the executable `path` (its parent must exist), so
/// `path 600` sleeps 600 seconds and the process is named `path`'s file name.
/// Its source goes in `env`'s root, so `path`'s directory holds only `path`.
///
/// `rustc` runs with the test's own environment, not the allowlisted one
/// fixture processes get: it needs the toolchain's (rustup's, the linker's)
/// variables and never reaches Rex.
pub fn build_sleeper(env: &TestEnv, path: &Path) {
    let source = env.root().join("sleeper.rs");
    std::fs::write(&source, SOURCE).unwrap();
    let output = Command::new("rustc")
        .args(["--edition", "2024", "-o"])
        .arg(path)
        .arg(&source)
        .output()
        .expect("rustc runs");
    assert!(
        output.status.success(),
        "building the sleeper failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
