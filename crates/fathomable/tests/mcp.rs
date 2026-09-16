//! Behavior-level tests for the repository-bound MCP review contract.

use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, ensure};
use fathomable_core::XdgDirs;
use fathomable_core::annotations::{Author, Draft, LineRange, Reply, Status, Store, ThreadId};
use fathomable_core::clock::now;
use fathomable_core::session::{Id, Record, Request, Response};
use fathomable_core::workspace::Workspace;
use fathomable_testing::TempDir;
use serde_json::{Value, json};

struct Fixture {
    dir: TempDir,
    dirs: XdgDirs,
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Result<Self> {
        let dir = TempDir::new(name)?;
        let root = dir.0.join("repository");
        fs::create_dir(&root)?;
        fs::write(root.join("a.md"), "one\ntwo\n")?;
        let root = root.canonicalize()?;
        let dirs = XdgDirs::resolve(|name| {
            (name == "XDG_STATE_HOME").then(|| dir.0.join("state").into_os_string())
        });
        Ok(Self { dir, dirs, root })
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fathomable"));
        command
            .current_dir(&self.dir.0)
            .env("HOME", &self.dir.0)
            .env("XDG_CONFIG_HOME", self.dir.0.join("config"))
            .env("XDG_STATE_HOME", self.dir.0.join("state"))
            .env_remove("XDG_RUNTIME_DIR")
            .env_remove("COPILOT_AGENT_SESSION_ID")
            .env_remove("CLAUDE_CODE_SESSION_ID");
        command
    }

    fn store(&self) -> Result<Store> {
        Ok(Store::open(self.dirs.threads_file(&self.root))?)
    }

    fn user_thread(&self, body: &str) -> Result<ThreadId> {
        Ok(Store::open(self.dirs.threads_file(&self.root))?.annotate(
            Draft::new(Author::User, Path::new("a.md"), LineRange::new(1, 1), body),
            "one\ntwo\n",
            now(),
        )?)
    }
}

struct Mcp {
    child: Child,
    input: ChildStdin,
    output: Receiver<std::io::Result<String>>,
    sequence: u64,
    meta: Value,
}

impl Mcp {
    fn copilot(fixture: &Fixture, session: &str) -> Result<Self> {
        Self::start(
            fixture,
            "copilot-cli",
            &[("COPILOT_AGENT_SESSION_ID", OsStr::new(session))],
        )
    }

    fn start(fixture: &Fixture, client: &str, env: &[(&str, &OsStr)]) -> Result<Self> {
        let mut command = fixture.command();
        command
            .arg("--mcp")
            .arg(&fixture.root)
            .envs(env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = command.spawn()?;
        let input = child.stdin.take().context("MCP stdin")?;
        let stdout = child.stdout.take().context("MCP stdout")?;
        let (send, output) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        let mut handle = Self {
            child,
            input,
            output,
            sequence: 0,
            meta: json!({}),
        };
        handle.request(
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": client, "version": "test"}
            }),
        )?;
        writeln!(
            handle.input,
            "{}",
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
        )?;
        Ok(handle)
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.sequence += 1;
        let mut request = json!({"jsonrpc": "2.0", "id": self.sequence, "method": method});
        request["params"] = params;
        writeln!(self.input, "{request}")?;
        loop {
            let line = self.output.recv_timeout(Duration::from_secs(10))??;
            let response: Value = serde_json::from_str(&line)?;
            if response.get("id") == Some(&json!(self.sequence)) {
                ensure!(response.get("error").is_none(), "{response}");
                return Ok(response["result"].clone());
            }
        }
    }

    fn call(&mut self, name: &str, arguments: Value) -> Result<Value> {
        let mut params = json!({"name": name, "_meta": self.meta});
        params["arguments"] = arguments;
        self.request("tools/call", params)
    }

    fn ok(&mut self, name: &str, arguments: Value) -> Result<Value> {
        let result = self.call(name, arguments)?;
        ensure!(result["isError"] != true, "{name}: {result}");
        Ok(result)
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn partial_reply_viewer(
    fixture: &Fixture,
) -> Result<(PathBuf, std::thread::JoinHandle<Result<()>>)> {
    let socket = fixture.dir.0.join("viewer.sock");
    let listener = UnixListener::bind(&socket)?;
    listener.set_nonblocking(true)?;
    Record::new(
        Id::mint(),
        fixture.root.clone(),
        fixture.root.clone(),
        Some(socket.clone()),
    )
    .write(&fixture.dirs)?;
    let store_path = fixture.dirs.threads_file(&fixture.root);
    let handle = std::thread::spawn(move || serve_partial_replies(&listener, &store_path));
    Ok((socket, handle))
}

fn serve_partial_replies(listener: &UnixListener, store_path: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut handled = 0;
    while handled < 2 && Instant::now() < deadline {
        let (mut stream, _) = match listener.accept() {
            Ok(connection) => connection,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        let mut line = String::new();
        BufReader::new(stream.try_clone()?).read_line(&mut line)?;
        let request: Request = line.trim_end().parse().map_err(anyhow::Error::msg)?;
        let response = if handled == 0 {
            let Request::ThreadReply {
                thread,
                author,
                body,
                resolve,
                lines,
            } = request
            else {
                return Err(anyhow!("expected a thread reply"));
            };
            ensure!(lines.is_none(), "test reply unexpectedly re-anchored");
            let mut store = Store::open(store_path)?;
            let reply = Reply::new(author, now(), body);
            store.reply(
                &thread,
                if resolve {
                    reply.proposing_resolution()
                } else {
                    reply
                },
            )?;
            Response::Threads(vec![
                store
                    .thread(&thread)
                    .context("first replied thread")?
                    .clone(),
            ])
        } else {
            Response::Error("injected later viewer failure".to_owned())
        };
        writeln!(stream, "{}", response.to_line())?;
        handled += 1;
    }
    ensure!(handled == 2, "fake viewer handled only {handled} requests");
    Ok(())
}

#[test]
fn exposes_only_three_tools_with_array_only_write_inputs() -> Result<()> {
    let fixture = Fixture::new("mcp-tools")?;
    let mut client = Mcp::copilot(&fixture, "chat")?;
    let listed = client.request("tools/list", json!({}))?;
    let tools = listed["tools"].as_array().context("tools")?;
    let mut names: Vec<&str> = tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["thread_reply", "thread_start", "threads"]);
    for (name, required) in [("thread_start", "comments"), ("thread_reply", "replies")] {
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == name)
            .context(name)?;
        assert_eq!(tool["inputSchema"]["required"], json!([required]));
        let properties = tool["inputSchema"]["properties"]
            .as_object()
            .context("properties")?;
        assert_eq!(properties.len(), 1, "{name}: {properties:?}");
        assert!(properties.contains_key(required));
    }
    Ok(())
}

#[test]
fn reading_returns_every_open_conversation_without_mutating_state() -> Result<()> {
    let fixture = Fixture::new("mcp-read")?;
    let first = fixture.user_thread("please explain")?;
    let second = fixture.user_thread("another question")?;
    let mut store = fixture.store()?;
    store.reply(
        &second,
        Reply::new(Author::agent("Earlier agent"), now(), "my proposal"),
    )?;
    drop(store);
    let mut client = Mcp::start(&fixture, "unknown-client", &[])?;
    let path = fixture.dirs.threads_file(&fixture.root);
    let before = fs::read(&path)?;
    let result = client.ok("threads", json!({}))?;
    let threads = result["structuredContent"]["threads"]
        .as_array()
        .context("threads")?;
    assert_eq!(threads.len(), 2);
    assert!(
        threads
            .iter()
            .any(|thread| thread["id"] == first.to_string())
    );
    let answered = threads
        .iter()
        .find(|thread| thread["id"] == second.to_string())
        .context("answered thread")?;
    assert_eq!(answered["comment"], "another question");
    assert_eq!(answered["replies"][0]["body"], "my proposal");
    assert_eq!(fs::read(path)?, before, "reading changed annotation state");
    Ok(())
}

#[test]
fn paging_progresses_across_equal_update_timestamps() -> Result<()> {
    let fixture = Fixture::new("mcp-page-ties")?;
    let mut store = fixture.store()?;
    for index in 0..51 {
        store.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(1, 1),
                format!("discussion {index}"),
            ),
            "one\ntwo\n",
            7,
        )?;
    }
    drop(store);
    let mut client = Mcp::start(&fixture, "unknown-client", &[])?;
    let first = client.ok("threads", json!({"limit": 50}))?;
    let first_threads = first["structuredContent"]["threads"]
        .as_array()
        .context("first threads")?;
    assert_eq!(first_threads.len(), 50);
    assert_eq!(first["structuredContent"]["more"], 1);
    let after = first["structuredContent"]["next_after"].clone();
    assert!(after.is_object(), "{after}");
    let second = client.ok("threads", json!({"limit": 50, "after": after}))?;
    let second_threads = second["structuredContent"]["threads"]
        .as_array()
        .context("second threads")?;
    assert_eq!(second_threads.len(), 1);
    assert_eq!(second["structuredContent"]["more"], 0);
    assert!(second["structuredContent"]["next_after"].is_null());
    assert!(
        first_threads
            .iter()
            .all(|thread| thread["id"] != second_threads[0]["id"])
    );
    Ok(())
}

#[test]
fn startup_housekeeping_preserves_placement_without_read_side_effects() -> Result<()> {
    let fixture = Fixture::new("mcp-read-placement")?;
    let original = "one\ntwo\nthree\n";
    fs::write(fixture.root.join("a.md"), original)?;
    let thread = Store::open(fixture.dirs.threads_file(&fixture.root))?.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(2, 2),
            "this line",
        ),
        original,
        1,
    )?;
    let mut seen = fathomable_core::seen::Store::open(&fixture.dirs.seen_dir(&fixture.root))?;
    seen.record(Path::new("a.md"), original)?;
    drop(seen);
    fs::write(fixture.root.join("a.md"), "zero\none\nTWO\nthree\n")?;

    let mut client = Mcp::start(&fixture, "unknown-client", &[])?;
    let path = fixture.dirs.threads_file(&fixture.root);
    let after_startup = fs::read(&path)?;
    let result = client.ok("threads", json!({"ids": [thread]}))?;
    let shown = &result["structuredContent"]["threads"][0];
    assert_eq!(shown["range"], json!({"start": 3, "end": 3}));
    assert_eq!(shown["placement"], "edited");
    assert_eq!(
        fs::read(path)?,
        after_startup,
        "threads call persisted housekeeping"
    );
    Ok(())
}

#[test]
fn resolved_and_exact_reads_keep_full_history_and_id_order() -> Result<()> {
    let fixture = Fixture::new("mcp-history")?;
    let first = fixture.user_thread("first question")?;
    let second = fixture.user_thread("second question")?;
    let mut store = fixture.store()?;
    store.reply(
        &first,
        Reply::new(Author::agent("Copilot"), now(), "I propose the rename.").proposing_resolution(),
    )?;
    store.resolve(&first, None, now())?;
    drop(store);
    let mut client = Mcp::start(&fixture, "unknown-client", &[])?;
    let resolved = client.ok("threads", json!({"status": "resolved"}))?;
    assert_eq!(
        resolved["structuredContent"]["threads"][0]["replies"][0]["body"],
        "I propose the rename."
    );
    let exact = client.ok(
        "threads",
        json!({"ids": [second.to_string(), first.to_string()]}),
    )?;
    let threads = exact["structuredContent"]["threads"]
        .as_array()
        .context("threads")?;
    assert_eq!(threads[0]["id"], second.to_string());
    assert_eq!(threads[1]["id"], first.to_string());
    assert_eq!(threads[1]["status"], "resolved");
    assert_eq!(
        threads[1]["replies"][0]["proposed_resolved"], true,
        "proposal metadata was lost"
    );
    Ok(())
}

#[test]
fn writes_require_identity_preserve_harness_authorship_and_user_only_closure() -> Result<()> {
    let fixture = Fixture::new("mcp-authorship")?;
    let thread = fixture.user_thread("why?")?;
    let mut anonymous = Mcp::start(&fixture, "unknown-client", &[])?;
    anonymous.ok("threads", json!({}))?;
    assert_eq!(
        anonymous.call(
            "thread_start",
            json!({"comments": [{"path": "a.md", "body": "not written"}]})
        )?["isError"],
        true
    );

    let mut client = Mcp::copilot(&fixture, "chat")?;
    client.ok(
        "thread_start",
        json!({"comments": [{"path": "a.md", "line": 2, "body": "new finding"}]}),
    )?;
    client.ok(
        "thread_reply",
        json!({"replies": [{"thread": thread, "body": "my proposal", "resolve": true}]}),
    )?;
    let store = fixture.store()?;
    let started = store.threads().last().context("started")?;
    assert_eq!(started.author().name(), "Copilot");
    assert_eq!(started.author().id(), Some("copilot:chat"));
    assert_eq!(started.author().kind(), None);
    let replied = store.thread(&thread).context("replied")?;
    assert_eq!(replied.replies()[0].author().id(), Some("copilot:chat"));
    assert!(replied.replies()[0].proposes_resolution());
    assert_eq!(replied.status(), Status::Open, "agent closed the thread");
    Ok(())
}

#[test]
fn batch_validation_writes_nothing_when_any_item_is_invalid() -> Result<()> {
    let fixture = Fixture::new("mcp-batch")?;
    let thread = fixture.user_thread("answer me")?;
    let mut client = Mcp::copilot(&fixture, "chat")?;
    let start = client.call(
        "thread_start",
        json!({"comments": [
            {"path": "a.md", "body": "valid"},
            {"path": "missing.md", "body": "invalid"}
        ]}),
    )?;
    assert_eq!(start["isError"], true);
    assert_eq!(fixture.store()?.threads().len(), 1);

    let reply = client.call(
        "thread_reply",
        json!({"replies": [
            {"thread": thread, "body": "valid"},
            {"thread": "not-a-thread", "body": "invalid"}
        ]}),
    )?;
    assert_eq!(reply["isError"], true);
    assert!(
        fixture
            .store()?
            .thread(&thread)
            .context("thread")?
            .replies()
            .is_empty()
    );
    Ok(())
}

#[test]
fn file_wide_range_override_invalidates_the_whole_reply_batch() -> Result<()> {
    let fixture = Fixture::new("mcp-file-wide-batch")?;
    let line_thread = fixture.user_thread("line question")?;
    let file_thread = Store::open(fixture.dirs.threads_file(&fixture.root))?.annotate(
        Draft::on_file(Author::User, Path::new("a.md"), "file question"),
        "one\ntwo\n",
        now(),
    )?;
    let mut client = Mcp::copilot(&fixture, "chat")?;
    let result = client.call(
        "thread_reply",
        json!({"replies": [
            {"thread": line_thread, "body": "valid first reply"},
            {"thread": file_thread, "body": "invalid placement", "line": 1}
        ]}),
    )?;
    assert_eq!(result["isError"], true);
    let store = fixture.store()?;
    assert!(
        store
            .thread(&line_thread)
            .context("line thread")?
            .replies()
            .is_empty()
    );
    assert!(
        store
            .thread(&file_thread)
            .context("file thread")?
            .replies()
            .is_empty()
    );
    Ok(())
}

#[test]
fn later_runtime_failure_reports_replies_already_written() -> Result<()> {
    let fixture = Fixture::new("mcp-partial-reply")?;
    let first = fixture.user_thread("first question")?;
    let second = fixture.user_thread("second question")?;
    let (socket, fake) = partial_reply_viewer(&fixture)?;

    let mut client = Mcp::copilot(&fixture, "chat")?;
    let result = client.call(
        "thread_reply",
        json!({"replies": [
            {"thread": first, "body": "first answer"},
            {"thread": second, "body": "second answer"}
        ]}),
    )?;
    assert_eq!(result["isError"], true);
    let summary = result["content"][0]["text"]
        .as_str()
        .context("error summary")?;
    assert!(
        summary.contains(&format!("replied to {first}")),
        "{summary}"
    );
    assert!(
        summary.contains("injected later viewer failure"),
        "{summary}"
    );
    fake.join()
        .map_err(|_panic| anyhow!("fake viewer thread panicked"))??;
    fs::remove_file(&socket)?;
    let store = fixture.store()?;
    assert_eq!(
        store
            .thread(&first)
            .context("first thread")?
            .replies()
            .len(),
        1
    );
    assert!(
        store
            .thread(&second)
            .context("second thread")?
            .replies()
            .is_empty()
    );
    Ok(())
}

#[test]
fn removed_single_item_and_workspace_arguments_are_rejected() -> Result<()> {
    let fixture = Fixture::new("mcp-removed-arguments")?;
    let thread = fixture.user_thread("why?")?;
    let mut client = Mcp::copilot(&fixture, "chat")?;
    for (tool, arguments) in [
        (
            "thread_start",
            json!({"path": "a.md", "body": "old single form"}),
        ),
        (
            "thread_reply",
            json!({"thread": thread, "body": "old single form"}),
        ),
        ("threads", json!({"workspace": fixture.root})),
        (
            "thread_start",
            json!({"workspace": fixture.root, "comments": [{"path": "a.md", "body": "x"}]}),
        ),
        (
            "thread_reply",
            json!({"workspace": fixture.root, "replies": [{"thread": thread, "body": "x"}]}),
        ),
    ] {
        let result = client.call(tool, arguments)?;
        assert_eq!(result["isError"], true, "{tool}: {result}");
    }
    assert_eq!(fixture.store()?.threads().len(), 1);
    Ok(())
}

#[test]
fn per_call_metadata_keeps_supported_chat_identities_distinct() -> Result<()> {
    for (label, client_name) in [
        ("vscode", "Visual Studio Code"),
        ("codex", "codex-mcp-client"),
    ] {
        let fixture = Fixture::new(&format!("mcp-{label}"))?;
        let mut client = Mcp::start(&fixture, client_name, &[])?;
        for chat in ["a", "b"] {
            client.meta = if label == "vscode" {
                json!({"vscode.conversationId": chat})
            } else {
                json!({"sessionId": "family", "threadId": chat})
            };
            client.ok(
                "thread_start",
                json!({"comments": [{"path": "a.md", "body": chat}]}),
            )?;
        }
        let store = fixture.store()?;
        assert_eq!(
            store.threads()[0].author().id(),
            Some(format!("{label}:a").as_str())
        );
        assert_eq!(
            store.threads()[1].author().id(),
            Some(format!("{label}:b").as_str())
        );
        assert_eq!(store.threads()[0].author().kind(), None);
    }
    Ok(())
}

#[test]
fn launch_identity_uses_only_the_selected_harness_channel() -> Result<()> {
    for (client_name, variable, expected) in [
        ("copilot-cli", "COPILOT_AGENT_SESSION_ID", "copilot:chat"),
        ("claude-code", "CLAUDE_CODE_SESSION_ID", "claude:chat"),
    ] {
        let fixture = Fixture::new(&format!("mcp-launch-{client_name}"))?;
        let mut client = Mcp::start(&fixture, client_name, &[(variable, OsStr::new("chat"))])?;
        client.ok(
            "thread_start",
            json!({"comments": [{"path": "a.md", "body": "identified"}]}),
        )?;
        assert_eq!(fixture.store()?.threads()[0].author().id(), Some(expected));
    }

    let fixture = Fixture::new("mcp-no-cross-harness")?;
    let mut client = Mcp::start(
        &fixture,
        "claude-code",
        &[("COPILOT_AGENT_SESSION_ID", OsStr::new("not-claude"))],
    )?;
    client.ok("threads", json!({}))?;
    assert_eq!(
        client.call(
            "thread_start",
            json!({"comments": [{"path": "a.md", "body": "refused"}]})
        )?["isError"],
        true
    );
    assert!(fixture.store()?.threads().is_empty());
    Ok(())
}

#[test]
fn per_request_client_info_selects_the_identity_adapter() -> Result<()> {
    let fixture = Fixture::new("mcp-request-client")?;
    let mut client = Mcp::copilot(&fixture, "launch-chat")?;
    client.meta = json!({
        "io.modelcontextprotocol/clientInfo": {
            "name": "Visual Studio Code",
            "version": "test"
        },
        "vscode.conversationId": "request-chat"
    });
    client.ok(
        "thread_start",
        json!({"comments": [{"path": "a.md", "body": "request identity"}]}),
    )?;
    assert_eq!(
        fixture.store()?.threads()[0].author().id(),
        Some("vscode:request-chat")
    );
    Ok(())
}

#[test]
fn legacy_annotation_history_and_agent_state_are_not_reset() -> Result<()> {
    let fixture = Fixture::new("mcp-existing-history")?;
    let thread = fixture.user_thread("historical question")?;
    let mut store = fixture.store()?;
    store.reply(
        &thread,
        Reply::new(
            Author::agent("Builder").subscribed("old-session", "reviewer"),
            now(),
            "historical answer",
        ),
    )?;
    drop(store);
    let legacy_register = fixture
        .dirs
        .workspace_dir(&fixture.root)
        .join("agents.jsonl");
    fs::write(&legacy_register, b"legacy agent state\n")?;

    let mut client = Mcp::start(&fixture, "unknown-client", &[])?;
    let result = client.ok("threads", json!({"ids": [thread]}))?;
    let author = &result["structuredContent"]["threads"][0]["replies"][0]["author"];
    assert_eq!(author["name"], "Builder");
    assert_eq!(author["id"], "old-session");
    assert_eq!(author["kind"], "reviewer");
    assert_eq!(fs::read(legacy_register)?, b"legacy agent state\n");
    Ok(())
}

#[test]
fn exact_ids_retrieve_resolved_history_hidden_by_checkout_status() -> Result<()> {
    let fixture = Fixture::new("mcp-old-resolved")?;
    fathomable_testing::git::init(&fixture.root)?;
    fathomable_testing::git::commit_and_stage(&fixture.root, &[("a.md", "one\ntwo\n")])?;
    let workspace = Workspace::discover(&fixture.root)?;
    let first_head = workspace.head_commit().context("first HEAD")?;
    let mut store = Store::open(fixture.dirs.threads_file(workspace.key()))?;
    let thread = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "old resolved question",
        )
        .at_commit(Some(first_head.clone())),
        "one\ntwo\n",
        now(),
    )?;
    store.resolve(&thread, Some(&first_head), now())?;
    drop(store);

    fs::write(fixture.root.join("a.md"), "one\ntwo\nthree\n")?;
    fathomable_testing::git::commit_and_stage(&fixture.root, &[("a.md", "one\ntwo\nthree\n")])?;
    let mut client = Mcp::start(&fixture, "unknown-client", &[])?;
    let ordinary = client.ok("threads", json!({"status": "resolved"}))?;
    assert_eq!(
        ordinary["structuredContent"]["threads"]
            .as_array()
            .context("ordinary threads")?
            .len(),
        0
    );
    let exact = client.ok("threads", json!({"ids": [thread]}))?;
    assert_eq!(
        exact["structuredContent"]["threads"][0]["comment"],
        "old resolved question"
    );
    Ok(())
}

#[test]
fn review_live_head_rewrites_keep_open_discussions_visible() -> Result<()> {
    let fixture = Fixture::new("mcp-live-amend")?;
    fathomable_testing::git::init(&fixture.root)?;
    let original = "one\ntwo\nthree\n";
    fs::write(fixture.root.join("a.md"), original)?;
    fathomable_testing::git::commit_and_stage(&fixture.root, &[("a.md", original)])?;
    let workspace = Workspace::discover(&fixture.root)?;
    let path = fixture.dirs.threads_file(workspace.key());
    let mut store = Store::open(&path)?;
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(2, 2),
            "keep this",
        )
        .at_commit(workspace.head_commit()),
        original,
        now(),
    )?;
    drop(store);
    fathomable_core::seen::Store::open(&fixture.dirs.seen_dir(workspace.key()))?
        .record(Path::new("a.md"), original)?;
    let mut client = Mcp::copilot(&fixture, "chat")?;
    client.ok("threads", json!({}))?;

    let rewritten = "zero\none\nTWO\nthree\n";
    fs::write(fixture.root.join("a.md"), rewritten)?;
    fathomable_testing::git::amend(&fixture.root, &[("a.md", rewritten)])?;
    let before = fs::read(&path)?;
    let result = client.ok("threads", json!({}))?;
    let shown = result["structuredContent"]["threads"]
        .as_array()
        .context("threads")?
        .iter()
        .find(|thread| thread["id"] == id.to_string())
        .with_context(|| format!("edited discussion disappeared after an amend: {result}"))?;
    assert_eq!(shown["range"], json!({"start": 3, "end": 3}));
    assert_eq!(shown["placement"], "edited");
    assert_eq!(fs::read(&path)?, before, "read persisted a rescope");
    client.ok(
        "thread_reply",
        json!({"replies": [{"thread": id, "body": "still reviewing"}]}),
    )?;
    assert_eq!(
        Store::open(path)?
            .thread(&id)
            .context("replied thread")?
            .replies()
            .len(),
        1
    );
    Ok(())
}

#[test]
fn review_live_rewrites_report_current_placement_without_mutation() -> Result<()> {
    let fixture = Fixture::new("mcp-live-placement")?;
    let original = "one\ntwo\nthree\n";
    fs::write(fixture.root.join("a.md"), original)?;
    let path = fixture.dirs.threads_file(&fixture.root);
    let id = Store::open(&path)?.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(2, 2),
            "this line",
        ),
        original,
        now(),
    )?;
    fathomable_core::seen::Store::open(&fixture.dirs.seen_dir(&fixture.root))?
        .record(Path::new("a.md"), original)?;
    let mut client = Mcp::copilot(&fixture, "chat")?;
    fs::write(fixture.root.join("a.md"), "zero\none\nTWO\nthree\n")?;
    let before = fs::read(&path)?;

    let result = client.ok("threads", json!({"ids": [id]}))?;
    let shown = &result["structuredContent"]["threads"][0];
    assert_eq!(shown["range"], json!({"start": 3, "end": 3}));
    assert_eq!(shown["placement"], "edited");
    assert_eq!(fs::read(path)?, before, "read persisted a relocation");
    Ok(())
}

#[test]
fn review_shared_repository_threads_keep_their_worktree_placement() -> Result<()> {
    let fixture = Fixture::new("mcp-other-worktree")?;
    fathomable_testing::git::init(&fixture.root)?;
    fathomable_testing::git::commit_and_stage(&fixture.root, &[("a.md", "one\ntwo\n")])?;
    let mut client = Mcp::copilot(&fixture, "chat")?;
    client.ok("threads", json!({}))?;

    let linked = fixture.dir.0.join("feature");
    fathomable_testing::git::worktree_add(&fixture.root, &linked, "feature")?;
    fs::write(linked.join("feature.md"), "branch-only finding\n")?;
    fathomable_testing::git::commit_and_stage(
        &linked,
        &[
            ("a.md", "one\ntwo\n"),
            ("feature.md", "branch-only finding\n"),
        ],
    )?;
    let workspace = Workspace::discover(&linked)?;
    let id = Store::open(fixture.dirs.threads_file(workspace.key()))?.annotate(
        Draft::new(
            Author::User,
            Path::new("feature.md"),
            LineRange::new(1, 1),
            "review this branch",
        )
        .at_commit(workspace.head_commit()),
        "branch-only finding\n",
        now(),
    )?;
    let result = client.ok("threads", json!({}))?;
    let shown = result["structuredContent"]["threads"]
        .as_array()
        .context("threads")?
        .iter()
        .find(|thread| thread["id"] == id.to_string())
        .context("shared worktree discussion missing")?;
    assert_eq!(shown["placement"], "anchored");
    assert_eq!(shown["worktree"], linked.to_string_lossy().as_ref());

    let filtered = client.ok("threads", json!({"path": "feature.md"}))?;
    let filtered = &filtered["structuredContent"]["threads"][0];
    assert_eq!(filtered["id"], id.to_string());
    let exact = client.ok("threads", json!({"ids": [id]}))?;
    let exact = &exact["structuredContent"]["threads"][0];
    for field in ["id", "path", "range", "placement", "worktree"] {
        assert_eq!(exact[field], shown[field], "exact lookup changed {field}");
    }

    fs::write(
        linked.join("feature.md"),
        "new branch-only preface\nbranch-only finding\n",
    )?;
    client.ok(
        "thread_reply",
        json!({"replies": [{
            "thread": id,
            "body": "updated on the feature branch",
            "line": 2
        }]}),
    )?;
    let store = Store::open(fixture.dirs.threads_file(workspace.key()))?;
    let thread = store.thread(&id).context("replied branch thread")?;
    assert_eq!(thread.author(), &Author::User);
    assert_eq!(thread.replies().len(), 1);
    assert_eq!(thread.replies()[0].author().id(), Some("copilot:chat"));
    assert_eq!(thread.range(), Some(LineRange::new(2, 2)));
    assert!(
        !fixture.root.join("feature.md").exists(),
        "reply created or required the sibling-only file in the bound checkout"
    );
    Ok(())
}

#[test]
fn review_startup_adopts_existing_root_keyed_annotations() -> Result<()> {
    let fixture = Fixture::new("mcp-root-state")?;
    fathomable_testing::git::init(&fixture.root)?;
    fathomable_testing::git::commit_and_stage(&fixture.root, &[("a.md", "one\ntwo\n")])?;
    let id = fixture.user_thread("existing review")?;
    let original = fs::read(fixture.dirs.threads_file(&fixture.root))?;
    let workspace = Workspace::discover(&fixture.root)?;
    let mut client = Mcp::start(&fixture, "unknown-client", &[])?;
    let result = client.ok("threads", json!({}))?;
    assert_eq!(
        result["structuredContent"]["threads"][0]["id"],
        id.to_string(),
        "startup opened an empty store instead of adopting existing annotations"
    );
    assert_eq!(
        fs::read(fixture.dirs.threads_file(workspace.key()))?,
        original
    );
    Ok(())
}
