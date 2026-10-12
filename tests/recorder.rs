//! The zsh recorder: which commands become a block's last command.
//!
//! Recorded commands are observed the way a user sees them: save, kill,
//! restore, and read the restored block's prompt.

mod common;

use std::path::Path;

use common::files::has_entries;
use common::server::wait_for_file;
use common::*;

/// Saves, kills the session labelled `label`, restores, and waits until each
/// restored block (in layout order) shows its expected command typed.
fn assert_restored_with_typed(env: &TestEnv, server: &Server, label: &str, expected: &[&str]) {
    env.save(server);
    server.kill_session(&server.the_session_labelled(label));
    env.restore(server);
    let restored = server.the_session_labelled(label);
    let blocks = server.block_ids(&restored);
    assert_eq!(blocks.len(), expected.len());
    for (block, command) in blocks.iter().zip(expected) {
        server.wait_for_typed(&restored, block, command);
    }
}

#[test]
fn a_shell_outside_rex_records_nothing() {
    let env = TestEnv::with_recorder();
    env.zsh_outside_rex(&[], "echo zz-outside\n");
    assert!(
        !has_entries(&env.state_dir()),
        "nothing written without REX_BLOCK"
    );

    // The same shell inside a block (REX_BLOCK set) does record.
    env.zsh_outside_rex(&[("REX_BLOCK", "block:zz-control")], "echo zz-inside\n");
    assert!(has_entries(&env.state_dir()), "the control run recorded");
}

#[test]
fn a_multi_line_command_leaves_the_previous_last_command_in_place() {
    let env = TestEnv::with_recorder();
    let server = env.start_server();
    let a = env.mkdir("a");
    let fixture = server.create_session("zz-multi", &[window(shell(&a))]);
    server.wait_for_pwds(&fixture);
    let block = fixture.block_ids()[0];
    let first = format!("touch {}", a.join("first").display());
    server.run(&fixture.id, block, &first);
    wait_for_file(&a.join("first"));
    // `&&` at the end of a line continues the command on the next one.
    let second = a.join("second");
    server.run(
        &fixture.id,
        block,
        &format!("true &&\ntouch {}", second.display()),
    );
    wait_for_file(&second);

    assert_restored_with_typed(&env, &server, "zz-multi", &[&first]);
}

#[test]
fn a_command_run_in_a_nested_shell_does_not_replace_the_command_that_started_it() {
    let env = TestEnv::with_recorder();
    let server = env.start_server();
    let a = env.mkdir("a");
    let fixture = server.create_session("zz-nested", &[window(shell(&a))]);
    server.wait_for_pwds(&fixture);
    let block = fixture.block_ids()[0];
    // The nested zsh loads the recorder too (same harness .zshrc).
    server.run(&fixture.id, block, "zsh");
    let inner = a.join("inner");
    server.run(&fixture.id, block, &format!("touch {}", inner.display()));
    wait_for_file(&inner);

    assert_restored_with_typed(&env, &server, "zz-nested", &["zsh"]);
}

#[test]
fn after_exec_zsh_the_outermost_shell_still_records() {
    let env = TestEnv::with_recorder();
    let server = env.start_server();
    let a = env.mkdir("a");
    let fixture = server.create_session("zz-exec", &[window(shell(&a))]);
    server.wait_for_pwds(&fixture);
    let block = fixture.block_ids()[0];
    server.run(&fixture.id, block, "exec zsh");
    let after = format!("touch {}", a.join("after").display());
    server.run(&fixture.id, block, &after);
    wait_for_file(&a.join("after"));

    assert_restored_with_typed(&env, &server, "zz-exec", &[&after]);
}

/// What oh-my-zsh does with `plugins=(...)` (oh-my-zsh.sh: `is_plugin`, the
/// fpath loop and `_omz_source`): for each plugin found as
/// `$ZSH_CUSTOM/plugins/<name>/<name>.plugin.zsh`, add its directory to
/// `fpath`, then source that file from inside a function with locals.
const OMZ_LIKE_LOADER: &str = r#"ZSH_CUSTOM=${ZDOTDIR:h}/omz-custom
plugins=(rex-resurrect)
for plugin ($plugins); do
  [[ -f $ZSH_CUSTOM/plugins/$plugin/$plugin.plugin.zsh ]] && fpath=($ZSH_CUSTOM/plugins/$plugin $fpath)
done
_omz_like_source() {
  local context filepath=$1
  local -A aliases_pre galiases_pre
  [[ -f $ZSH_CUSTOM/$filepath ]] && source $ZSH_CUSTOM/$filepath
}
for plugin ($plugins); do
  _omz_like_source plugins/$plugin/$plugin.plugin.zsh
done
unset plugin"#;

#[test]
fn the_recorder_loads_as_an_oh_my_zsh_custom_plugin() {
    let env = TestEnv::builder().zshrc_line(OMZ_LIKE_LOADER).build();
    let plugin_dir = env.root().join("omz-custom/plugins/rex-resurrect");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::copy(
        common::zsh::recorder_plugin(),
        plugin_dir.join("rex-resurrect.plugin.zsh"),
    )
    .unwrap();
    let server = env.start_server();
    let a = env.mkdir("a");
    let fixture = server.create_session("zz-omz", &[window(shell(&a))]);
    server.wait_for_pwds(&fixture);
    let command = format!("touch {}", a.join("omz").display());
    server.run(&fixture.id, fixture.block_ids()[0], &command);
    wait_for_file(&a.join("omz"));

    assert_restored_with_typed(&env, &server, "zz-omz", &[&command]);
}

#[test]
fn an_ignored_command_leaves_the_previous_last_command_in_place() {
    let env = TestEnv::with_recorder();
    let server = env.start_server();
    let a = env.mkdir("a");
    let sub = env.mkdir("a/sub");
    std::fs::write(sub.join("zz-listed"), "").unwrap();
    let fixture = server.create_session("zz-ignored", &[window(shell(&a))]);
    server.wait_for_pwds(&fixture);
    let block = fixture.block_ids()[0];
    let meaningful = format!("touch {}", a.join("meaningful").display());
    server.run(&fixture.id, block, &meaningful);
    wait_for_file(&a.join("meaningful"));
    // Ignored by default, with or without arguments.
    server.run(&fixture.id, block, "cd sub");
    server.wait_for_pwd_eq(&fixture.id, block, &sub);
    server.run(&fixture.id, block, "clear");
    server.run(&fixture.id, block, "ls -a");
    server.wait_for_capture(&fixture.id, block, "the ls output", |c| {
        c.contains("zz-listed")
    });

    assert_restored_with_typed(&env, &server, "zz-ignored", &[&meaningful]);
}

/// An ignore list entry matches the command as typed or the last part of
/// its path, and rex-resurrect is on the list by default: running it is
/// never what a block was doing.
#[test]
fn rex_resurrect_and_ignored_commands_run_by_path_leave_the_previous_last_command_in_place() {
    let env = TestEnv::with_recorder();
    let server = env.start_server();
    let a = env.mkdir("a");
    std::fs::write(a.join("zz-listed"), "").unwrap();
    let fixture = server.create_session("zz-by-path", &[window(shell(&a))]);
    server.wait_for_pwds(&fixture);
    let block = fixture.block_ids()[0];
    let touched = env.root().join("zz-touched");
    let meaningful = format!("touch {}", touched.display());
    server.run(&fixture.id, block, &meaningful);
    wait_for_file(&touched);
    server.run(
        &fixture.id,
        block,
        &format!("{} list", common::resurrect::bin()),
    );
    server.wait_for_capture(&fixture.id, block, "rex-resurrect's list", |c| {
        c.contains("\nno snapshots yet")
    });
    server.run(&fixture.id, block, "/bin/ls");
    server.wait_for_capture(&fixture.id, block, "the ls output", |c| {
        c.contains("\nzz-listed")
    });

    assert_restored_with_typed(&env, &server, "zz-by-path", &[&meaningful]);
}

#[test]
fn a_command_line_joining_an_ignored_command_to_another_is_recorded() {
    let env = TestEnv::with_recorder();
    let server = env.start_server();
    let a = env.mkdir("a");
    env.mkdir("a/sub");
    let fixture = server.create_session("zz-joined", &[window(shell(&a))]);
    server.wait_for_pwds(&fixture);
    let joined = format!("cd sub && touch {}", a.join("joined").display());
    server.run(&fixture.id, fixture.block_ids()[0], &joined);
    wait_for_file(&a.join("joined"));

    assert_restored_with_typed(&env, &server, "zz-joined", &[&joined]);
}

/// Runs `commands` in order in a new session's one block, waiting for each
/// to show it ran: a touched file, or `echo`'s and `ls`'s output (a file
/// `zz-listed` sits in the block's directory for `ls`).
fn run_in_one_block(env: &TestEnv, server: &Server, label: &str, commands: &[&str]) {
    let a = env.mkdir("a");
    std::fs::write(a.join("zz-listed"), "").unwrap();
    let fixture = server.create_session(label, &[window(shell(&a))]);
    server.wait_for_pwds(&fixture);
    let block = fixture.block_ids()[0];
    for command in commands {
        server.run(&fixture.id, block, command);
        if let Some(path) = command.strip_prefix("touch ") {
            wait_for_file(Path::new(path));
        } else if let Some(text) = command.strip_prefix("echo ") {
            server.wait_for_capture(&fixture.id, block, text, |c| {
                c.contains(&format!("\n{text}"))
            });
        } else {
            server.wait_for_capture(&fixture.id, block, "the ls output", |c| {
                c.contains("\nzz-listed")
            });
        }
    }
}

#[test]
fn users_can_extend_the_ignore_list_from_their_zsh_config() {
    let plugin = common::zsh::quote(&common::zsh::recorder_plugin());
    let env = TestEnv::builder()
        .zshrc_line(format!("source {plugin}"))
        .zshrc_line("REX_RESURRECT_IGNORE+=(echo)")
        .build();
    let server = env.start_server();
    let touch = format!("touch {}", env.root().join("zz-touched").display());
    // `ls` is still ignored by default, and `echo` now is too.
    run_in_one_block(
        &env,
        &server,
        "zz-extend",
        &[&touch, "ls", "echo zz-extended"],
    );

    assert_restored_with_typed(&env, &server, "zz-extend", &[&touch]);
}

#[test]
fn users_can_replace_the_ignore_list_from_their_zsh_config() {
    let plugin = common::zsh::quote(&common::zsh::recorder_plugin());
    let env = TestEnv::builder()
        .zshrc_line("REX_RESURRECT_IGNORE=(echo)")
        .zshrc_line(format!("source {plugin}"))
        .build();
    let server = env.start_server();
    let touch = format!("touch {}", env.root().join("zz-touched").display());
    // `ls` is no longer ignored; `echo` is.
    run_in_one_block(
        &env,
        &server,
        "zz-replace",
        &[&touch, "ls", "echo zz-replaced"],
    );

    assert_restored_with_typed(&env, &server, "zz-replace", &["ls"]);
}
