//! Agent conversations: a block running pi, Claude Code or Codex comes back
//! running that agent again, resuming the conversation it had open.
//!
//! The agents are stand-ins ([`FakeAgents`]) on the blocks' `PATH`: each
//! logs how it was started, reports a conversation through
//! `rexurrect agent-hook` the way the real agent's hook does, then waits
//! under the agent's name, as the real one would sit at its prompt.

mod common;

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use common::server::wait_for;
use common::*;

const AGENTS: [&str; 3] = ["pi", "claude", "codex"];

/// Stand-in `pi`, `claude` and `codex` in `<root>/bin`, which every fixture
/// block puts first on its `PATH`. Each appends to `<root>/agent-log/<name>`
/// one line per start: `started <args>` then `conversation <ref>`.
struct FakeAgents {
    root: PathBuf,
    state_dir: PathBuf,
}

impl FakeAgents {
    /// A `TestEnv` with the zsh recorder and the stand-in agents.
    fn env() -> (TestEnv, FakeAgents) {
        let env = TestEnv::builder()
            .zshrc_line(format!(
                "source {}",
                common::zsh::quote(&common::zsh::recorder_plugin())
            ))
            .zshrc_line(r#"path=("${REXURRECT_STATE_DIR:h}/bin" $path)"#)
            .build();
        let agents = FakeAgents {
            root: env.root().to_path_buf(),
            state_dir: env.state_dir(),
        };
        for dir in ["bin", "agent-log", "pi-sessions"] {
            std::fs::create_dir_all(agents.root.join(dir)).unwrap();
        }
        for name in AGENTS {
            let script = agents.root.join("bin").join(name);
            std::fs::write(&script, agents.script(name)).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        (env, agents)
    }

    fn script(&self, name: &str) -> String {
        let root = self.root.display();
        let rexurrect = common::rexurrect::bin();
        // How each agent is told which conversation to resume, and what its
        // hook sends on stdin.
        let (resume_flag, input) = match name {
            "pi" => ("--session", r#"{"session_file":"%s"}"#),
            "claude" => (
                "--resume",
                r#"{"hook_event_name":"SessionStart","session_id":"%s"}"#,
            ),
            "codex" => (
                "resume",
                r#"{"hook_event_name":"SessionStart","session_id":"%s"}"#,
            ),
            _ => unreachable!(),
        };
        let new_conversation = if name == "pi" {
            format!("{root}/pi-sessions/$$.jsonl")
        } else {
            format!("{name}-$$")
        };
        format!(
            r#"#!/bin/bash
# Stand-in {name} (rexurrect test harness).
log='{root}/agent-log/{name}'
echo "started $*" >> "$log"
if [ "$1" = '{resume_flag}' ]; then c=$2; else c="{new_conversation}"; fi
[ '{name}' = pi ] && touch "$c"
echo "conversation $c" >> "$log"
printf '{input}' "$c" | '{rexurrect}' agent-hook {name}
exec -a {name} sleep 100000
"#
        )
    }

    fn log(&self, name: &str) -> Vec<String> {
        match std::fs::read_to_string(self.root.join("agent-log").join(name)) {
            Ok(log) => log.lines().map(str::to_string).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// The lines starting with `started`: how each run of `name` was started.
    fn starts(&self, name: &str) -> Vec<String> {
        self.log(name)
            .into_iter()
            .filter_map(|line| line.strip_prefix("started ").map(str::to_string))
            .collect()
    }

    /// Waits until `name` has been started `n` times and has reported its
    /// conversation each time, and returns the last conversation.
    fn wait_for_runs(&self, name: &str, n: usize) -> String {
        wait_for(&format!("{name} to have run {n} time(s)"), || {
            let conversations: Vec<String> = self
                .log(name)
                .into_iter()
                .filter_map(|line| line.strip_prefix("conversation ").map(str::to_string))
                .collect();
            (conversations.len() >= n).then(|| conversations[n - 1].clone())
        })
    }

    /// Runs `command` in a block to start `name`, its `runs`th run, and
    /// waits until its hook has reported. Returns the conversation.
    fn start(
        &self,
        server: &Server,
        session_id: &str,
        block_id: &str,
        command: &str,
        name: &str,
        runs: usize,
    ) -> String {
        server.run(session_id, block_id, command);
        let conversation = self.wait_for_runs(name, runs);
        self.wait_for_report(block_id, &conversation);
        server.wait_for_foreground(session_id, block_id, "sleep");
        conversation
    }

    /// Waits until `name`'s hook has recorded `conversation` for `block_id`.
    fn wait_for_report(&self, block_id: &str, conversation: &str) {
        let file = self.state_dir.join("agents").join(block_id);
        wait_for(
            &format!("{block_id}'s agent file to name {conversation}"),
            || {
                std::fs::read_to_string(&file)
                    .ok()
                    .filter(|text| text.contains(conversation))
                    .map(|_| ())
            },
        );
    }
}

#[test]
fn each_agent_comes_back_resuming_its_conversation() {
    let (env, agents) = FakeAgents::env();
    let server = env.start_server();
    let dirs = AGENTS.map(|name| env.mkdir(name));
    let fixture = server.create_session(
        "zz-agents",
        &[window(hsplit(
            0.5,
            shell(&dirs[0]),
            vsplit(0.5, shell(&dirs[1]), shell(&dirs[2])),
        ))],
    );
    server.wait_for_pwds(&fixture);
    let blocks = fixture.block_ids();
    let conversations: Vec<String> = AGENTS
        .iter()
        .zip(&blocks)
        .map(|(name, block)| agents.start(&server, &fixture.id, block, name, name, 1))
        .collect();

    env.save(&server);
    server.kill_session(&fixture.id);
    env.restore(&server);

    for (name, conversation) in AGENTS.iter().zip(&conversations) {
        agents.wait_for_runs(name, 2);
        let resumed = match *name {
            "pi" => format!("--session {conversation}"),
            "claude" => format!("--resume {conversation}"),
            _ => format!("resume {conversation}"),
        };
        assert_eq!(agents.starts(name), ["".to_string(), resumed], "{name}");
    }
    // Each in its own block, in its saved directory.
    let restored = server.the_session_labelled("zz-agents");
    for (block, dir) in server.block_ids(&restored).iter().zip(&dirs) {
        server.wait_for_pwd_eq(&restored, block, dir);
    }
}

#[test]
fn a_resumed_conversation_is_resumed_again_after_the_next_restart() {
    let (env, agents) = FakeAgents::env();
    let server = env.start_server();
    let fixture = server.create_session("zz-again", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    let conversation = agents.start(&server, &fixture.id, fixture.block_ids()[0], "pi", "pi", 1);
    env.save(&server);
    server.kill_session(&fixture.id);
    env.restore(&server);
    agents.wait_for_runs("pi", 2);

    // Once in the restored session's startup window, before the resumed
    // agent has reported (its hook runs once it has started), and once after
    // the window, from its report.
    for (runs, end_window) in [(3, false), (4, true)] {
        let restored = server.the_session_labelled("zz-again");
        let block = &server.block_ids(&restored)[0];
        server.wait_for_foreground(&restored, block, "sleep");
        agents.wait_for_report(block, &conversation);
        if end_window {
            env.end_startup_windows();
        } else {
            std::fs::remove_file(env.state_dir().join("agents").join(block)).unwrap();
        }
        env.save(&server);
        server.kill_session(&restored);
        env.restore(&server);
        assert_eq!(agents.wait_for_runs("pi", runs), conversation);
    }
    let resumed = format!("--session {conversation}");
    assert_eq!(
        agents.starts("pi")[1..],
        [resumed.clone(), resumed.clone(), resumed]
    );
}

#[test]
fn a_block_whose_agent_has_exited_gets_its_last_command_typed_not_run() {
    let (env, agents) = FakeAgents::env();
    let server = env.start_server();
    let fixture = server.create_session("zz-exited", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    let block = fixture.block_ids()[0];
    agents.start(&server, &fixture.id, block, "claude", "claude", 1);
    server.write(&fixture.id, block, "\x03");
    server.wait_for_empty_prompt(&fixture.id, block);

    env.save(&server);
    server.kill_session(&fixture.id);
    env.restore(&server);

    let restored = server.the_session_labelled("zz-exited");
    server.wait_for_typed(&restored, &server.block_ids(&restored)[0], "claude");
    assert_eq!(agents.starts("claude").len(), 1, "claude wasn't run again");
}

#[test]
fn a_conversation_open_in_two_blocks_is_resumed_in_one() {
    let (env, agents) = FakeAgents::env();
    let server = env.start_server();
    let [a, b] = ["a", "b"].map(|name| env.mkdir(name));
    let fixture = server.create_session("zz-twice", &[window(hsplit(0.5, shell(&a), shell(&b)))]);
    server.wait_for_pwds(&fixture);
    let command = "claude --resume zz-shared";
    for (i, block) in fixture.block_ids().into_iter().enumerate() {
        agents.start(&server, &fixture.id, block, command, "claude", i + 1);
    }

    env.save(&server);
    server.kill_session(&fixture.id);
    env.restore(&server);

    agents.wait_for_runs("claude", 3);
    let restored = server.the_session_labelled("zz-twice");
    server.wait_for_typed(&restored, &server.block_ids(&restored)[1], command);
    assert_eq!(
        agents.starts("claude").len(),
        3,
        "resumed in one block only"
    );
}

#[test]
fn a_pi_conversation_whose_session_file_is_gone_is_not_resumed() {
    let (env, agents) = FakeAgents::env();
    let server = env.start_server();
    let fixture = server.create_session("zz-gone", &[window(shell(env.mkdir("a")))]);
    server.wait_for_pwds(&fixture);
    let file = agents.start(&server, &fixture.id, fixture.block_ids()[0], "pi", "pi", 1);

    env.save(&server);
    server.kill_session(&fixture.id);
    std::fs::remove_file(&file).unwrap();
    env.restore(&server);

    let restored = server.the_session_labelled("zz-gone");
    server.wait_for_typed(&restored, &server.block_ids(&restored)[0], "pi");
    assert_eq!(agents.starts("pi").len(), 1, "pi wasn't run again");
}

#[test]
fn a_conversation_whose_directory_is_gone_is_not_resumed() {
    let (env, agents) = FakeAgents::env();
    let server = env.start_server();
    let dir = env.mkdir("doomed");
    let fixture = server.create_session("zz-nodir", &[window(shell(&dir))]);
    server.wait_for_pwds(&fixture);
    agents.start(
        &server,
        &fixture.id,
        fixture.block_ids()[0],
        "codex",
        "codex",
        1,
    );

    env.save(&server);
    server.kill_session(&fixture.id);
    std::fs::remove_dir_all(&dir).unwrap();
    env.restore(&server);

    let restored = server.the_session_labelled("zz-nodir");
    server.wait_for_typed(&restored, &server.block_ids(&restored)[0], "codex");
    assert_eq!(agents.starts("codex").len(), 1, "codex wasn't run again");
}

/// Runs `rexurrect agent-hook <agent>` with `input` on stdin and `vars` set,
/// as an agent runs its hook.
fn run_hook(
    env: &TestEnv,
    agent: &str,
    vars: &[(&str, &str)],
    input: &str,
) -> std::process::Output {
    let mut child = Command::new(common::rexurrect::bin())
        .args(["agent-hook", agent])
        .env_clear()
        .envs(env.child_env())
        .envs(vars.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn agents_dir(env: &TestEnv) -> PathBuf {
    env.state_dir().join("agents")
}

fn agent_file(env: &TestEnv, block: &str) -> Option<String> {
    std::fs::read_to_string(agents_dir(env).join(block)).ok()
}

const BLOCK: &str = "block:01a1zz00-0000-7000-8000-000000000001";

#[test]
fn the_hook_prints_nothing_and_records_the_conversation_privately() {
    let env = TestEnv::new();
    let input = r#"{"hook_event_name":"SessionStart","session_id":"zz-1","source":"startup"}"#;

    let out = run_hook(&env, "claude", &[("REX_BLOCK", BLOCK)], input);

    assert!(out.status.success(), "{out:?}");
    // Claude Code adds a SessionStart hook's output to the conversation.
    assert!(out.stdout.is_empty(), "{out:?}");
    assert!(agent_file(&env, BLOCK).unwrap().contains("zz-1"));
    assert_eq!(
        std::fs::metadata(agents_dir(&env))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

#[test]
fn the_hook_records_nothing_outside_rex() {
    let env = TestEnv::new();
    let input = r#"{"hook_event_name":"SessionStart","session_id":"zz-1"}"#;

    let out = run_hook(&env, "claude", &[], input);

    assert!(out.status.success(), "{out:?}");
    assert!(!agents_dir(&env).exists());
}

#[test]
fn the_hook_follows_the_conversation_on_screen() {
    let env = TestEnv::new();
    let start = |id: &str| format!(r#"{{"hook_event_name":"SessionStart","session_id":"{id}"}}"#);
    run_hook(&env, "claude", &[("REX_BLOCK", BLOCK)], &start("zz-first"));

    // `/clear` starts a new conversation in the same block.
    run_hook(&env, "claude", &[("REX_BLOCK", BLOCK)], &start("zz-second"));
    // A subagent's session isn't the block's.
    let subagent = r#"{"hook_event_name":"SessionStart","session_id":"zz-sub","agent_id":"a1"}"#;
    run_hook(&env, "claude", &[("REX_BLOCK", BLOCK)], subagent);
    // Nor is a Codex started by another Codex.
    run_hook(
        &env,
        "codex",
        &[("REX_BLOCK", BLOCK), ("CODEX_THREAD_ID", "zz-parent")],
        &start("zz-child"),
    );

    assert!(agent_file(&env, BLOCK).unwrap().contains("zz-second"));
}

#[test]
fn pi_starting_a_session_it_does_not_save_leaves_the_block_without_a_conversation() {
    let env = TestEnv::new();
    let file = env.mkdir("s").join("a.jsonl");
    let input = format!(r#"{{"session_file":"{}"}}"#, file.display());
    run_hook(&env, "pi", &[("REX_BLOCK", BLOCK)], &input);
    assert!(agent_file(&env, BLOCK).is_some());

    run_hook(&env, "pi", &[("REX_BLOCK", BLOCK)], "{}");

    assert!(agent_file(&env, BLOCK).is_none());
}

#[test]
fn an_unknown_agent_is_a_usage_error() {
    let env = TestEnv::new();
    let out = run_hook(&env, "aider", &[("REX_BLOCK", BLOCK)], "{}");
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        Path::new(&env.state_dir())
            .read_dir()
            .unwrap()
            .next()
            .is_none()
    );
}
