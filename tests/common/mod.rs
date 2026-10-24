//! Test harness shared by every integration test.
//!
//! Tests drive the built `rexurrect` binary against a throwaway Rex server
//! and assert only on what Rex reports (`session.view`, `pwd`,
//! `rex capture`), never on the snapshot file's format.
//!
//! - [`TestEnv`]: one test's temp world (state dir, harness `ZDOTDIR`,
//!   scratch dirs). Start servers from it and run the binary through it.
//! - [`Server`]: a throwaway `rex server run` with its own short socket path,
//!   `--state-dir` and init.lua, plus the client calls tests need (create,
//!   view, write, pwd, capture, kill, rename). Stopped on drop.
//! - [`fixture`]: layouts to create (`window`, `hsplit`, `vsplit`, `shell`)
//!   and [`Shape`], the comparable form of a layout Rex reports.
//! - [`RexurrectCmd`]: one run of the binary with a scrubbed environment.
//! - [`files`]: the state directory's files as seen from outside (modes,
//!   total size, whether a directory holds anything, their bytes to compare),
//!   for privacy, retention and "wrote nothing" checks that don't depend on
//!   file names or formats.
//!
//! Shared steps and checks: [`TestEnv::saved_on_an_earlier_server`] (a
//! snapshot taken before a restart), [`TestEnv::listed`],
//! [`Server::sorted_labels`], [`Server::assert_empty_prompt`].
//!
//! Safety: every child process that can reach Rex gets an environment built
//! from an allowlist, so `REX_BLOCK`, `REX_SESSION`, the real `REX_SERVER` and
//! the user's XDG dirs never leak in. Harness `rex` calls always pass `-S <endpoint>` and
//! `--autostart=false`. Nothing touches process-global state, so tests can run
//! concurrently; each test starts its own server(s).
//!
//! Seams for later tests:
//! - recorder: `TestEnv::builder().zshrc_line("source …").build()`;
//! - custom init.lua: `env.server().init_lua("…").start()`; a server whose
//!   init.lua keeps it from being ready: `.start_without_waiting()`, then
//!   [`Server::wait_until_ready`];
//! - stub `rex`: [`RexStub`], put first on one run's `PATH` with
//!   `env.rexurrect(&server).path_prepend(stub.dir())` (the harness itself
//!   always uses the real `rex`, see [`server::rex_bin`]); a slow one:
//!   [`stub::slow_rex`]. Auto-restore runs the `rex` its server runs, not
//!   the one on `PATH`: start that server with
//!   `.shown_as(stub.dir().join("rex"))`.
//! - an executable to copy, rename or run through a symlink:
//!   [`build_sleeper`], a test-built program that sleeps.

#![allow(dead_code, unused_imports)]

pub mod env;
pub mod files;
pub mod fixture;
pub mod rexurrect;
pub mod server;
pub mod sleeper;
pub mod stub;
pub mod tempdir;
pub mod zsh;

pub use env::{APP_SESSION_WATCH_VAR, STATE_DIR_VAR, SYSTEM_PATH, TestEnv};
pub use fixture::{FixtureSession, Layout, Shape, Window, block, hsplit, shell, vsplit, window};
pub use fixture::{WindowLabels, default_window_label, labels};
pub use rexurrect::{RexurrectCmd, RunOutput, Running};
pub use server::{Server, SessionSummary};
pub use sleeper::build_sleeper;
pub use stub::RexStub;
