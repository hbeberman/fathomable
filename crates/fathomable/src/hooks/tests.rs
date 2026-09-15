//! The hooks' tests: a sibling module, as the source policy has it.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

use fathomable_core::XdgDirs;
use fathomable_core::agents::Register;
use fathomable_core::annotations::{Author, Draft, LineRange, Reply, Store};
use fathomable_core::config::AgentsConfig;
use fathomable_core::session::Marker;
use fathomable_core::workspace::Workspace;
use serde_json::json;

use super::{Bound, Diag, Event, Harness, Input, Occasion, compose, pending_input, workspace_for};
use fathomable_core::clock::now;
use fathomable_testing::TempDir;
use fathomable_testing::git;

use crate::app::testing;

type TestResult = Result<(), Box<dyn Error>>;

fn fixture(name: &str) -> std::io::Result<TempDir> {
    let dir = testing::bare(&format!("hooks-{name}"))?;
    fs::create_dir_all(dir.0.join("ws/src"))?;
    Ok(dir)
}

fn dirs(dir: &TempDir) -> XdgDirs {
    let state = dir.0.join("state").into_os_string();
    let config = dir.0.join("config").into_os_string();
    XdgDirs::resolve(move |name| match name {
        "XDG_STATE_HOME" => Some(state.clone()),
        "XDG_CONFIG_HOME" => Some(config.clone()),
        _ => None,
    })
}

#[test]
fn hooks_deliver_only_to_the_matching_harness_subscription() -> TestResult {
    for (harness, key) in [
        (Harness::Claude, "claude:s-1"),
        (Harness::Copilot, "copilot:s-1"),
        (Harness::Codex, "codex:s-1"),
        (Harness::Vscode, "vscode:s-1"),
    ] {
        let dir = fixture(&format!("qualified-{harness:?}"))?;
        let dirs = dirs(&dir);
        let root = dir.0.join("ws").canonicalize()?;
        Marker::new(root.clone(), vec![root.clone()]).write(&dirs)?;
        let mut store = Store::open(dirs.threads_file(&root))?;
        store.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(1, 1),
                "why?",
            ),
            "one\n",
            now(),
        )?;
        let payload = if harness == Harness::Copilot {
            json!({"sessionId": "s-1", "cwd": root})
        } else {
            json!({"session_id": "s-1", "cwd": root})
        };
        let run = || {
            let mut diag = Diag::new(true, "pending", Some(harness));
            diag.flushed = true;
            let code = pending_input(
                &dirs,
                Some(harness),
                None,
                false,
                Input::parse(harness, &payload),
                &mut diag,
            );
            (code, diag.lines.clone())
        };
        let (code, lines) = run();
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(
            lines
                .iter()
                .any(|line| line == "silent: nothing to deliver")
        );
        assert!(
            !dirs.agents_file(&root).exists(),
            "no implicit subscription"
        );

        let other = if harness == Harness::Copilot {
            "claude:s-1"
        } else {
            "copilot:s-1"
        };
        let expiry = AgentsConfig::default().expire_after;
        let mut register = Register::open(dirs.agents_file(&root), now(), expiry)?;
        register.subscribe(other, "coder", None, None, now())?;
        let before = fs::read(register.path())?;
        let (code, lines) = run();
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(
            lines
                .iter()
                .any(|line| line == "silent: nothing to deliver")
        );
        assert_eq!(
            fs::read(register.path())?,
            before,
            "another harness stays untouched"
        );

        register.subscribe(key, "coder", None, None, now())?;
        let (code, lines) = run();
        let expected = match harness {
            Harness::Claude | Harness::Codex => ExitCode::from(2),
            Harness::Copilot | Harness::Vscode => ExitCode::SUCCESS,
        };
        assert_eq!(code, expected);
        assert!(lines.iter().any(|line| line.starts_with("answer: ")));
        let register = Register::open(register.path(), now(), expiry)?;
        let subscriber = register.subscriber(key).ok_or("missing subscriber")?;
        assert!(register.deliverable(subscriber, store.threads()).is_empty());
        let other = register
            .subscriber(other)
            .ok_or("missing other subscriber")?;
        assert_eq!(register.deliverable(other, store.threads()).len(), 1);
        let (code, lines) = run();
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(
            lines
                .iter()
                .any(|line| line == "silent: nothing to deliver")
        );
    }
    Ok(())
}

#[test]
fn explicit_hook_ids_are_native_and_manual_ids_are_qualified() -> TestResult {
    for (index, (harness, explicit, delivered)) in [
        (Some(Harness::Claude), "explicit", true),
        (None, "claude:explicit", true),
        (Some(Harness::Claude), " ", false),
        (None, "explicit", false),
        (None, "claude: ", false),
        (None, "unknown:explicit", false),
    ]
    .into_iter()
    .enumerate()
    {
        let dir = fixture(&format!("explicit-{index}"))?;
        let dirs = dirs(&dir);
        let root = dir.0.join("ws").canonicalize()?;
        Marker::new(root.clone(), vec![root.clone()]).write(&dirs)?;
        let mut store = Store::open(dirs.threads_file(&root))?;
        store.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(1, 1),
                "why?",
            ),
            "one\n",
            now(),
        )?;
        let expiry = AgentsConfig::default().expire_after;
        let path = dirs.agents_file(&root);
        let mut register = Register::open(&path, now(), expiry)?;
        register.subscribe("claude:explicit", "coder", None, None, now())?;
        let before = fs::read(&path)?;
        let input = Input::parse(
            Harness::Claude,
            &json!({"session_id": "stdin", "cwd": root}),
        );
        let mut diag = Diag::new(true, "pending", harness);
        diag.flushed = true;
        assert_eq!(
            pending_input(
                &dirs,
                harness,
                Some(explicit.to_owned()),
                true,
                input,
                &mut diag
            ),
            ExitCode::SUCCESS
        );
        let register = Register::open(&path, now(), expiry)?;
        let subscriber = register
            .subscriber("claude:explicit")
            .ok_or("missing subscriber")?;
        assert_eq!(
            register.deliverable(subscriber, store.threads()).is_empty(),
            delivered
        );
        if delivered {
            assert!(diag.lines.iter().any(|line| line.starts_with("answer: ")));
        } else {
            assert!(
                diag.lines
                    .iter()
                    .any(|line| line == "silent: invalid chat identity")
            );
            assert!(!diag.lines.iter().any(|line| line.starts_with("answer: ")));
            assert_eq!(fs::read(&path)?, before);
        }
    }
    Ok(())
}

/// A subscribed session is handed a thread once; the next check is
/// silent until the user speaks again; an unsubscribed one hears nothing.
#[test]
fn pending_delivers_each_message_once() -> TestResult {
    let dir = fixture("once")?;
    let dirs = dirs(&dir);
    let root = dir.0.join("ws").canonicalize()?;
    Marker::new(root.clone(), vec![root.clone()]).write(&dirs)?;
    assert_eq!(
        workspace_for(&dirs, &root.join("src")),
        Some(Bound::plain(root.clone()))
    );
    assert_eq!(workspace_for(&dirs, Path::new("/nowhere")), None);
    let bound = Bound::plain(root.clone());
    let mut store = Store::open(dirs.threads_file(&root))?;
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "why?",
        ),
        "one\n",
        5,
    )?;
    let config = AgentsConfig {
        nag_after: 2,
        ..AgentsConfig::default()
    };
    assert_eq!(
        compose(&dirs, &bound, "ghost", &config, "user", Occasion::TurnEnd)?,
        None
    );
    let when = now();
    let mut register = Register::open(dirs.agents_file(&root), when, config.expire_after)?;
    register.subscribe("s-1", "coder", Some("bot"), None, when)?;
    let text = compose(&dirs, &bound, "s-1", &config, "user", Occasion::TurnEnd)?
        .ok_or("nothing delivered")?;
    assert!(text.contains("1 review thread needs your reply"), "{text}");
    assert!(text.contains("user: why?"), "{text}");
    assert_eq!(
        compose(&dirs, &bound, "s-1", &config, "user", Occasion::TurnEnd)?,
        None,
        "first check is quiet"
    );
    let nag =
        compose(&dirs, &bound, "s-1", &config, "user", Occasion::TurnEnd)?.ok_or("no reminder")?;
    assert!(
        nag.starts_with("FATHOMABLE reminder: 1 thread still unanswered: a.md:1"),
        "{nag}"
    );
    assert_eq!(
        compose(&dirs, &bound, "s-1", &config, "user", Occasion::TurnEnd)?,
        None
    );
    let me = Author::agent("bot").subscribed("s-1", "coder");
    store.reply(&id, Reply::new(me, 7, "because"))?;
    assert_eq!(
        compose(&dirs, &bound, "s-1", &config, "user", Occasion::TurnEnd)?,
        None,
        "own reply"
    );
    store.reply(&id, Reply::new(Author::User, 8, "hmm"))?;
    let again =
        compose(&dirs, &bound, "s-1", &config, "user", Occasion::TurnEnd)?.ok_or("nothing")?;
    assert!(again.contains("user: hmm"), "{again}");
    Ok(())
}

/// A blob past `max-lines` lists the rest and tells the model to call
/// `threads` — so the rest must still be deliverable there.
#[test]
fn overflow_threads_survive_for_the_threads_tool() -> TestResult {
    let dir = fixture("overflow")?;
    let dirs = dirs(&dir);
    let root = dir.0.join("ws").canonicalize()?;
    Marker::new(root.clone(), vec![root.clone()]).write(&dirs)?;
    let bound = Bound::plain(root.clone());
    let mut store = Store::open(dirs.threads_file(&root))?;
    let text = "one\ntwo\nthree\n";
    let mut ids = Vec::new();
    for n in 0..4 {
        ids.push(store.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(1, 3),
                format!("why {n}?"),
            ),
            text,
            5,
        )?);
    }
    let config = AgentsConfig {
        max_lines: 20,
        ..AgentsConfig::default()
    };
    let when = now();
    let mut register = Register::open(dirs.agents_file(&root), when, config.expire_after)?;
    register.subscribe("s-1", "coder", Some("bot"), None, when)?;
    let blob = compose(&dirs, &bound, "s-1", &config, "user", Occasion::TurnEnd)?
        .ok_or("nothing delivered")?;
    assert!(blob.contains("more; call `threads`"), "{blob}");
    // A thread is shown in full only if it has a `── thread <id>`
    // header; the overflow appears by id alone in the closing list.
    let listed: Vec<_> = ids
        .iter()
        .filter(|id| !blob.contains(&format!("── thread {id} ")))
        .collect();
    assert!(!listed.is_empty(), "nothing overflowed: {blob}");

    // The blob told the model to fetch the rest; they must be there.
    let register = Register::open(dirs.agents_file(&root), when, config.expire_after)?;
    let subscriber = register.subscriber("s-1").ok_or("gone")?;
    let threads = super::scoped_threads(&dirs, &Bound::plain(root.clone()))?;
    let deliverable: Vec<_> = register
        .deliverable(subscriber, &threads)
        .into_iter()
        .map(|t| t.id().clone())
        .collect();
    for id in &listed {
        assert!(
            deliverable.contains(id),
            "{id} was listed as \"more; call threads\" but is already \
             recorded as delivered, so threads will not return it"
        );
    }
    Ok(())
}

/// A turn start hands the blob over as context, not as a
/// forced turn, so it must not spend the nag cadence that ADR 0040
/// counts in turn-ends — otherwise a harness that runs its
/// prompt-submit hook on every continuation would nag about threads
/// no turn ever refused to answer.
#[test]
fn context_does_not_count_toward_the_nag() -> TestResult {
    let dir = fixture("context")?;
    let dirs = dirs(&dir);
    let root = dir.0.join("ws").canonicalize()?;
    Marker::new(root.clone(), vec![root.clone()]).write(&dirs)?;
    let bound = Bound::plain(root.clone());
    let mut store = Store::open(dirs.threads_file(&root))?;
    store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "why?",
        ),
        "one\n",
        5,
    )?;
    let config = AgentsConfig {
        nag_after: 2,
        ..AgentsConfig::default()
    };
    let when = now();
    let mut register = Register::open(dirs.agents_file(&root), when, config.expire_after)?;
    register.subscribe("s-1", "coder", Some("bot"), None, when)?;
    assert!(compose(&dirs, &bound, "s-1", &config, "user", Occasion::Context)?.is_some());
    for _ in 0..5 {
        assert_eq!(
            compose(&dirs, &bound, "s-1", &config, "user", Occasion::Context)?,
            None,
            "context neither repeats the blob nor nags"
        );
    }
    // The cadence is untouched, so it still takes `nag_after`
    // turn-ends to earn the reminder.
    assert_eq!(
        compose(&dirs, &bound, "s-1", &config, "user", Occasion::TurnEnd)?,
        None
    );
    let nag = compose(&dirs, &bound, "s-1", &config, "user", Occasion::TurnEnd)?
        .ok_or("no reminder after two turn-ends")?;
    assert!(nag.starts_with("FATHOMABLE reminder:"), "{nag}");
    Ok(())
}

#[test]
fn subagents_are_told_apart_per_harness() {
    let claude = Input::parse(
        Harness::Claude,
        &json!({"session_id": "s", "cwd": "/w", "agent_id": "a", "stop_hook_active": true}),
    );
    assert!(claude.subagent && claude.continuation);
    assert_eq!(claude.session.as_deref(), Some("s"));
    assert_eq!(claude.event, Event::Stop);
    let main = Input::parse(Harness::Codex, &json!({"session_id": "s", "cwd": "/w"}));
    assert!(!main.subagent && !main.continuation);
    let copilot_main = Input::parse(
        Harness::Copilot,
        &json!({"sessionId": "p", "transcriptPath": "/x/session-state/p/events.jsonl"}),
    );
    assert!(!copilot_main.subagent);
    let copilot_sub = Input::parse(
        Harness::Copilot,
        &json!({"sessionId": "c", "transcriptPath": "/x/session-state/p/events.jsonl"}),
    );
    assert!(copilot_sub.subagent);
    let copilot_old = Input::parse(
        Harness::Copilot,
        &json!({"sessionId": "c", "transcriptPath": ""}),
    );
    assert!(copilot_old.subagent);
    let copilot_none = Input::parse(Harness::Copilot, &json!({"sessionId": "c"}));
    assert!(!copilot_none.subagent);
    assert_eq!(Input::parse(Harness::Vscode, &json!({})), Input::default());
}

/// `--verbose` off records nothing; on, it keeps what it is told.
#[test]
fn diag_records_only_when_on() {
    let mut quiet = Diag::new(false, "pending", None);
    quiet.note("lookup");
    assert!(quiet.lines.is_empty());
    let mut loud = Diag::new(true, "pending", Some(Harness::Copilot));
    loud.note("lookup");
    assert_eq!(loud.lines.len(), 2);
    assert!(loud.lines[0].contains("pending --hook copilot"));
    loud.flushed = true;
}

/// The context events are known by name where the harness gives one
/// and by their own fields where it does not (Copilot): `prompt` on a
/// prompt-submit, `toolName` after a tool. A stop payload has neither,
/// and a wake's notification XML is still a prompt.
#[test]
fn context_events_are_told_from_stop() {
    let parse = |h, v| Input::parse(h, &v).event;
    assert_eq!(
        parse(
            Harness::Claude,
            json!({"session_id": "s", "hook_event_name": "UserPromptSubmit", "prompt": "<task-notification>x</task-notification>"})
        ),
        Event::Prompt
    );
    assert_eq!(
        parse(
            Harness::Copilot,
            json!({"sessionId": "s", "cwd": "/w", "prompt": "hi"})
        ),
        Event::Prompt
    );
    assert_eq!(
        parse(
            Harness::Claude,
            json!({"session_id": "s", "hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_response": {}})
        ),
        Event::PostTool
    );
    assert_eq!(
        parse(
            Harness::Copilot,
            json!({"sessionId": "s", "toolName": "bash", "toolResult": {}})
        ),
        Event::PostTool
    );
    assert_eq!(
        parse(
            Harness::Claude,
            json!({"session_id": "s", "hook_event_name": "Stop", "last_assistant_message": "waiting", "stop_hook_active": false})
        ),
        Event::Stop
    );
    assert_eq!(
        parse(
            Harness::Copilot,
            json!({"sessionId": "s", "stopReason": "end_turn"})
        ),
        Event::Stop
    );
    assert_eq!(
        parse(
            Harness::Copilot,
            json!({"sessionId": "s", "hook_event_name": "Notification", "notification_type": "shell_detached_completed", "message": "done"})
        ),
        Event::Notification
    );
    // What Copilot 1.0.82 actually sends: no event name, camel case.
    assert_eq!(
        parse(
            Harness::Copilot,
            json!({"sessionId": "s", "notificationType": "shell_detached_completed", "message": "done", "title": "Shell finished"})
        ),
        Event::Notification
    );
    assert_eq!(
        parse(
            Harness::Copilot,
            json!({"sessionId": "s", "notificationType": "permission_prompt", "message": "Use MCP tool: fathomable/thread_reply", "title": "Permission needed"})
        ),
        Event::Other
    );
}

/// A cwd in a linked worktree binds to the repository's workspace at
/// that worktree: through the marker's roots when it names it, and
/// through git when the worktree was added after the marker was
/// written (ADR 0070).
#[test]
fn a_worktree_finds_its_workspace_with_or_without_the_marker() -> anyhow::Result<()> {
    let dir = TempDir::new("hooks-worktree")?;
    let dirs = dirs(&dir);
    let main = dir.0.join("main");
    fs::create_dir_all(&main)?;
    git::init(&main)?;
    git::commit_and_stage(&main, &[("a.md", "one\n")])?;
    let workspace = Workspace::discover(&main)?;
    let key = workspace.key().to_path_buf();
    let main = workspace.root().to_path_buf();
    Marker::new(key.clone(), vec![main.clone()]).write(&dirs)?;

    let feature = dir.0.join("feature");
    git::worktree_add(&main, &feature, "feature")?;
    let feature = feature.canonicalize()?;
    fs::create_dir_all(feature.join("src"))?;

    let bound =
        workspace_for(&dirs, &feature.join("src")).ok_or_else(|| anyhow::anyhow!("unbound"))?;
    assert_eq!(
        bound.key, key,
        "found through git, the marker not naming it"
    );
    assert_eq!(bound.root, feature);

    Marker::new(key.clone(), vec![main.clone(), feature.clone()]).write(&dirs)?;
    let bound =
        workspace_for(&dirs, &feature.join("src")).ok_or_else(|| anyhow::anyhow!("unbound"))?;
    assert_eq!(bound.root, feature, "and through the marker once it does");
    assert_eq!(bound.roots, vec![main.clone(), feature]);
    assert_eq!(workspace_for(&dirs, &main).map(|b| b.root), Some(main));
    Ok(())
}
