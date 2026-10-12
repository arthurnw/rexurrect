//! A stub `rex` for output no real server produces (a changed beta API).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::Value;

use super::env::TestEnv;
use super::server::rex_bin;

/// A directory holding a stub `rex` script. Put it first on one run's
/// `PATH` with `env.resurrect(&server).path_prepend(stub.dir())`.
///
/// The stub handles the `api call` invocations its rules pick (by method,
/// and optionally by something the JSON arguments on stdin contain) and
/// hands every other invocation, arguments and stdin unchanged, to the real
/// `rex`. So the binary under test still reaches the throwaway server
/// (through the `-S` it passes) for everything else, and a canned answer can
/// be Rex's real output with one thing changed. The first rule that matches
/// wins.
pub struct RexStub {
    dir: PathBuf,
    rules: Vec<Rule>,
}

/// Which `api call` invocations a rule applies to.
struct Matching {
    method: String,
    /// Only when the JSON arguments contain this text (a session's label or
    /// ID, say).
    input_contains: Option<String>,
}

enum Rule {
    /// Answers with the file's contents on stdout, exit 0.
    Answer { matching: Matching, file: PathBuf },
    /// Fails: `message` on stderr, exit `status`.
    Fail {
        matching: Matching,
        status: i32,
        message: String,
    },
    /// Runs the real `rex`, then pipes what it printed through `sed`.
    Rewrite { matching: Matching, sed: String },
    /// Runs the real `rex`, then the `sh` command `command`.
    After { matching: Matching, command: String },
}

impl RexStub {
    pub fn new(env: &TestEnv) -> RexStub {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = env.root().join(format!("stub{n}"));
        std::fs::create_dir_all(dir.join("answers")).unwrap();
        let stub = RexStub {
            dir,
            rules: Vec::new(),
        };
        stub.write_script();
        stub
    }

    /// Answers `rex ... api call <method> ...` with `result` on stdout, exit 0.
    pub fn answer(self, method: &str, result: &Value) -> RexStub {
        self.answer_matching(matching(method, None), result)
    }

    /// Like [`RexStub::answer`], only for calls whose JSON arguments contain
    /// `input_contains`.
    pub fn answer_when(self, method: &str, input_contains: &str, result: &Value) -> RexStub {
        self.answer_matching(matching(method, Some(input_contains)), result)
    }

    /// Fails calls to `method` whose JSON arguments contain `input_contains`
    /// the way `rex` does: `message` on stderr, exit `status`.
    pub fn fail_when(
        self,
        method: &str,
        input_contains: &str,
        status: i32,
        message: &str,
    ) -> RexStub {
        self.with_rule(Rule::Fail {
            matching: matching(method, Some(input_contains)),
            status,
            message: message.to_string(),
        })
    }

    /// For calls to `method` whose JSON arguments contain `input_contains`,
    /// runs the real `rex` and pipes its output through the `sed` script
    /// `sed` (exit status kept).
    pub fn rewrite_when(self, method: &str, input_contains: &str, sed: &str) -> RexStub {
        self.with_rule(Rule::Rewrite {
            matching: matching(method, Some(input_contains)),
            sed: sed.to_string(),
        })
    }

    /// After every call to `method`, which the real `rex` answers, runs the
    /// `sh` command `command` (the call's exit status kept): something that
    /// happens right after the server answered.
    pub fn after(self, method: &str, command: &str) -> RexStub {
        self.with_rule(Rule::After {
            matching: matching(method, None),
            command: command.to_string(),
        })
    }

    /// The directory to put first on `PATH`.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn answer_matching(self, matching: Matching, result: &Value) -> RexStub {
        let file = self
            .dir
            .join("answers")
            .join(format!("{}.json", self.rules.len()));
        std::fs::write(&file, format!("{result}\n")).unwrap();
        self.with_rule(Rule::Answer { matching, file })
    }

    fn with_rule(mut self, rule: Rule) -> RexStub {
        self.rules.push(rule);
        self.write_script();
        self
    }

    fn write_script(&self) {
        let script = self.dir.join("rex");
        std::fs::write(&script, stub_script(&self.rules, rex_bin())).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn matching(method: &str, input_contains: Option<&str>) -> Matching {
    Matching {
        method: method.to_string(),
        input_contains: input_contains.map(str::to_string),
    }
}

fn stub_script(rules: &[Rule], real_rex: &Path) -> String {
    let mut script = format!(
        r#"#!/bin/sh
# Stub rex (rex-resurrect test harness): rules for chosen `api call`
# invocations, everything else handed to the real rex.
real={real_rex}
method=
prev=
for arg in "$@"; do
  if [ "$prev" = call ]; then method=$arg; break; fi
  prev=$arg
done
[ -n "$method" ] || exec "$real" "$@"
input=$(cat)
"#,
        real_rex = sh_quote(real_rex),
    );
    for rule in rules {
        let (matching, action) = match rule {
            Rule::Answer { matching, file } => {
                (matching, format!("cat {}; exit 0", sh_quote(file)))
            }
            Rule::Fail {
                matching,
                status,
                message,
            } => (
                matching,
                format!("printf '%s\\n' {} >&2; exit {status}", sh_word(message)),
            ),
            Rule::After { matching, command } => (
                matching,
                format!(
                    r#"printf '%s' "$input" | "$real" "$@"; status=$?; {command}; exit $status"#
                ),
            ),
            Rule::Rewrite { matching, sed } => (
                matching,
                format!(
                    r#"out=$(printf '%s' "$input" | "$real" "$@") || exit $?; printf '%s\n' "$out" | sed -e {}; exit 0"#,
                    sh_word(sed)
                ),
            ),
        };
        let input_pattern = match &matching.input_contains {
            Some(text) => format!("*{}*", sh_word(text)),
            None => "*".to_string(),
        };
        script.push_str(&format!(
            "case \"$method\" in {method}) case \"$input\" in {input_pattern}) {action} ;; esac ;; esac\n",
            method = sh_word(&matching.method),
        ));
    }
    script.push_str("printf '%s' \"$input\" | exec \"$real\" \"$@\"\n");
    script
}

/// A directory holding a `rex` that sleeps `seconds` before every call to
/// `method`, then runs the real `rex`. Put it first on a run's `PATH` to make
/// a restore slow enough to overlap with another run.
pub fn slow_rex(env: &TestEnv, method: &str, seconds: f64) -> PathBuf {
    let dir = env.mkdir("slow-rex");
    let script = format!(
        "#!/bin/sh\nfor arg in \"$@\"; do\n  [ \"$arg\" = {method} ] && sleep {seconds}\ndone\nexec {} \"$@\"\n",
        sh_quote(rex_bin())
    );
    let rex = dir.join("rex");
    std::fs::write(&rex, script).unwrap();
    std::fs::set_permissions(&rex, std::fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

/// `path` as one single-quoted shell word.
pub fn sh_quote(path: &Path) -> String {
    sh_word(path.to_str().unwrap())
}

/// `text` as one single-quoted shell word.
pub fn sh_word(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}
