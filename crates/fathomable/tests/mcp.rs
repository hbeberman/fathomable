//! Automatic chat identity and workspace isolation at the stdio MCP boundary.

use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use fathomable_core::XdgDirs;
use fathomable_core::agents::Register;
use fathomable_core::annotations::{Author, Draft, LineRange, Store, ThreadId};
use fathomable_core::clock::now;
use fathomable_core::config::AgentsConfig;
use fathomable_core::session::{Id, Marker, Record};
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
        let root = dir.0.join("workspace");
        fs::create_dir(&root)?;
        fs::write(root.join("a.md"), "one\ntwo\n")?;
        let dirs = XdgDirs::resolve(|name| {
            (name == "XDG_STATE_HOME").then(|| dir.0.join("state").into_os_string())
        });
        Marker::new(root.clone(), vec![root.clone()]).write(&dirs)?;
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

    fn register(&self, root: &Path) -> Result<Register> {
        Ok(Register::open(
            self.dirs.agents_file(root),
            now(),
            AgentsConfig::default().expire_after,
        )?)
    }

    fn user_thread(&self) -> Result<ThreadId> {
        Ok(Store::open(self.dirs.threads_file(&self.root))?.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(1, 1),
                "why?",
            ),
            "one\ntwo\n",
            now(),
        )?)
    }

    fn store(&self, root: &Path) -> Result<Store> {
        Ok(Store::open(self.dirs.threads_file(root))?)
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
        let mut client_handle = Self {
            child,
            input,
            output,
            sequence: 0,
            meta: json!({}),
        };
        client_handle.request(
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": client, "version": "test"}
            }),
        )?;
        writeln!(
            client_handle.input,
            "{}",
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
        )?;
        Ok(client_handle)
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

#[test]
fn authorship_precedes_subscription_and_delivery_stays_opt_in() -> Result<()> {
    let fixture = Fixture::new("mcp-opt-in")?;
    let thread = fixture.user_thread()?;
    let mut client = Mcp::copilot(&fixture, "chat")?;
    client.ok("workspaces", json!({}))?;
    client.ok("threads", json!({"status": "pending"}))?;
    assert!(!fixture.dirs.agents_file(&fixture.root).exists());
    assert_eq!(client.call("follow", json!({}))?["isError"], true);
    client.ok("thread_reply", json!({"thread": thread, "body": "because"}))?;
    client.ok("thread_start", json!({"path": "a.md", "body": "new topic"}))?;
    let store = fixture.store(&fixture.root)?;
    assert_eq!(
        store.thread(&thread).context("thread")?.replies()[0]
            .author()
            .id(),
        Some("copilot:chat")
    );
    assert_eq!(store.threads()[1].author().id(), Some("copilot:chat"));
    assert_eq!(store.threads()[1].author().kind(), None);
    assert!(!fixture.dirs.agents_file(&fixture.root).exists());

    let followed = client.ok("follow", json!({"type": "coder", "persona": "Builder"}))?;
    assert!(
        followed
            .to_string()
            .contains(&fixture.root.display().to_string())
    );
    client.ok("follow", json!({"type": "coder"}))?;
    assert_eq!(
        fixture
            .register(&fixture.root)?
            .subscriber("copilot:chat")
            .context("subscriber")?
            .name(),
        Some("Builder")
    );
    assert_eq!(
        client.call("follow", json!({"type": "reviewer"}))?["isError"],
        true
    );
    let another = fixture.user_thread()?;
    client.ok("threads", json!({"status": "pending"}))?;
    let register = fixture.register(&fixture.root)?;
    let store = fixture.store(&fixture.root)?;
    let subscriber = register.subscriber("copilot:chat").context("subscriber")?;
    assert!(
        register
            .deliverable(subscriber, store.threads().iter())
            .is_empty()
    );
    client.ok("thread_watch", json!({"on": another, "when": "message"}))?;
    assert_eq!(fixture.register(&fixture.root)?.watches().len(), 1);
    client.ok("follow", json!({"end": true}))?;
    let register = fixture.register(&fixture.root)?;
    assert!(register.subscribers().is_empty() && register.watches().is_empty());
    Ok(())
}

#[test]
fn environment_adapters_separate_harnesses_and_chats_without_borrowing_another_harness_env()
-> Result<()> {
    let fixture = Fixture::new("mcp-environments")?;
    for (client_name, session, expected) in [
        ("copilot-cli", "a", "copilot:a"),
        ("claude-code", "a", "claude:a"),
        ("copilot-cli", "b", "copilot:b"),
        ("claude-code", "b", "claude:b"),
    ] {
        let mut client = Mcp::start(
            &fixture,
            client_name,
            &[
                ("COPILOT_AGENT_SESSION_ID", OsStr::new(session)),
                ("CLAUDE_CODE_SESSION_ID", OsStr::new(session)),
            ],
        )?;
        client.ok("thread_start", json!({"path": "a.md", "body": expected}))?;
        let store = fixture.store(&fixture.root)?;
        assert_eq!(
            store.threads().last().context("thread")?.author().id(),
            Some(expected)
        );
    }
    for client_name in [
        "claude-code",
        "Visual Studio Code",
        "codex",
        "unknown-client",
    ] {
        let mut client = Mcp::start(
            &fixture,
            client_name,
            &[("COPILOT_AGENT_SESSION_ID", OsStr::new("not-yours"))],
        )?;
        client.ok("threads", json!({}))?;
        assert_eq!(
            client.call(
                "thread_start",
                json!({"path": "a.md", "body": "must refuse"})
            )?["isError"],
            true
        );
    }
    assert_eq!(fixture.store(&fixture.root)?.threads().len(), 4);
    assert!(!fixture.dirs.agents_file(&fixture.root).exists());
    Ok(())
}

#[test]
fn per_call_metadata_isolates_chat_authors_roles_and_deliveries_on_one_connection() -> Result<()> {
    for (label, client_name) in [
        ("vscode", "Visual Studio Code"),
        ("codex", "codex-mcp-client"),
    ] {
        let fixture = Fixture::new(&format!("mcp-metadata-{label}"))?;
        let thread = fixture.user_thread()?;
        let mut client = Mcp::start(&fixture, client_name, &[])?;
        for (chat, role) in [("a", "coder"), ("b", "reviewer")] {
            client.meta = match label {
                "vscode" => {
                    json!({"vscode.conversationId": chat, "vscode.requestId": "same-request"})
                }
                _ => json!({"sessionId": "shared-family", "threadId": chat, "callId": "same-call"}),
            };
            client.ok("follow", json!({"type": role}))?;
            client.ok("thread_start", json!({"path": "a.md", "body": chat}))?;
            client.ok("threads", json!({"status": "pending"}))?;
            let register = fixture.register(&fixture.root)?;
            let store = fixture.store(&fixture.root)?;
            let subscriber = register
                .subscriber(&format!("{label}:{chat}"))
                .context("subscriber")?;
            assert!(
                register
                    .deliverable(subscriber, store.threads().iter())
                    .is_empty()
            );
            client.ok("thread_watch", json!({"on": thread, "when": "message"}))?;
        }
        let store = fixture.store(&fixture.root)?;
        assert_eq!(
            store.threads()[1].author().id(),
            Some(format!("{label}:a").as_str())
        );
        assert_eq!(store.threads()[1].author().kind(), Some("coder"));
        assert_eq!(
            store.threads()[2].author().id(),
            Some(format!("{label}:b").as_str())
        );
        assert_eq!(store.threads()[2].author().kind(), Some("reviewer"));
        assert_eq!(fixture.register(&fixture.root)?.watches().len(), 2);
        client.ok("follow", json!({"end": true}))?;
        let register = fixture.register(&fixture.root)?;
        assert!(register.subscriber(&format!("{label}:a")).is_some());
        assert!(register.subscriber(&format!("{label}:b")).is_none());
        assert_eq!(register.watches().len(), 1);

        client.meta = json!({});
        assert_eq!(
            client.call(
                "thread_reply",
                json!({"thread": thread, "body": "no cached identity"})
            )?["isError"],
            true
        );
        client.ok("threads", json!({}))?;
    }
    Ok(())
}

#[test]
fn metadata_changes_the_caller_without_changing_another_chats_workspace() -> Result<()> {
    let fixture = Fixture::new("mcp-chat-workspaces")?;
    let other = fixture.dir.0.join("other");
    fs::create_dir(&other)?;
    fs::write(other.join("a.md"), "other\n")?;
    Marker::new(other.clone(), vec![other.clone()]).write(&fixture.dirs)?;
    let mut client = Mcp::start(&fixture, "Visual Studio Code", &[])?;
    client.meta = json!({"vscode.conversationId": "a"});
    client.ok("follow", json!({"type": "coder"}))?;
    client.meta = json!({"vscode.conversationId": "b"});
    client.ok("follow", json!({"workspace": other, "type": "reviewer"}))?;
    client.ok(
        "thread_start",
        json!({"workspace": other, "path": "a.md", "body": "B elsewhere"}),
    )?;
    client.meta = json!({"vscode.conversationId": "a"});
    client.ok(
        "thread_start",
        json!({"path": "a.md", "body": "A still here"}),
    )?;
    client.ok(
        "thread_start",
        json!({"workspace": other, "path": "a.md", "body": "A not subscribed there"}),
    )?;
    let local = fixture.store(&fixture.root)?;
    assert_eq!(local.threads()[0].author().id(), Some("vscode:a"));
    assert_eq!(local.threads()[0].author().kind(), Some("coder"));
    let remote = fixture.store(&other)?;
    assert_eq!(remote.threads()[0].author().id(), Some("vscode:b"));
    assert_eq!(remote.threads()[0].author().kind(), Some("reviewer"));
    assert_eq!(remote.threads()[1].author().id(), Some("vscode:a"));
    assert_eq!(remote.threads()[1].author().kind(), None);
    let listed = client.ok("workspaces", json!({}))?;
    let workspaces = listed["structuredContent"]["workspaces"]
        .as_array()
        .context("workspaces")?;
    assert!(
        workspaces
            .iter()
            .any(|workspace| workspace["root"] == json!(fixture.root)
                && workspace["default"] == true)
    );
    Ok(())
}

#[test]
fn duplicate_viewer_names_do_not_choose_another_workspace() -> Result<()> {
    let fixture = Fixture::new("mcp-ambiguous-workspace")?;
    let other = fixture.dir.0.join("other");
    fs::create_dir(&other)?;
    Marker::new(other.clone(), vec![other.clone()]).write(&fixture.dirs)?;
    for (index, root) in [&fixture.root, &other].into_iter().enumerate() {
        Record::new(
            format!("1-{index}").parse::<Id>()?,
            root.clone(),
            root.clone(),
            None,
        )
        .with_name(Some("review".to_owned()))
        .write(&fixture.dirs)?;
    }
    let mut client = Mcp::copilot(&fixture, "chat")?;
    let result = client.call("follow", json!({"workspace": "review", "type": "coder"}))?;
    assert_eq!(result["isError"], true);
    assert!(result.to_string().contains("ambiguous viewer"));
    assert!(!fixture.dirs.agents_file(&fixture.root).exists());
    assert!(!fixture.dirs.agents_file(&other).exists());
    client.ok("follow", json!({"workspace": other, "type": "coder"}))?;
    assert!(
        fixture
            .register(&other)?
            .subscriber("copilot:chat")
            .is_some()
    );
    Ok(())
}

#[test]
fn per_request_client_identity_selects_the_adapter_instead_of_the_handshake() -> Result<()> {
    let fixture = Fixture::new("mcp-per-request-client")?;
    let mut client = Mcp::copilot(&fixture, "launch")?;
    client.meta = json!({
        "io.modelcontextprotocol/clientInfo": {"name": "Visual Studio Code", "version": "test"},
        "vscode.conversationId": "request-chat"
    });
    client.ok(
        "thread_start",
        json!({"path": "a.md", "body": "VS Code request"}),
    )?;
    assert_eq!(
        fixture.store(&fixture.root)?.threads()[0].author().id(),
        Some("vscode:request-chat")
    );
    Ok(())
}

#[test]
fn invalid_or_missing_identity_never_registers_or_posts_and_legacy_tool_ids_are_refused()
-> Result<()> {
    let fixture = Fixture::new("mcp-invalid-identity")?;
    for session in [None, Some(OsStr::new("")), Some(OsStr::new(" \t"))] {
        let env: Vec<_> = session
            .into_iter()
            .map(|id| ("COPILOT_AGENT_SESSION_ID", id))
            .collect();
        let mut client = Mcp::start(&fixture, "copilot-cli", &env)?;
        client.ok("threads", json!({}))?;
        assert_eq!(
            client.call("follow", json!({"type": "coder"}))?["isError"],
            true
        );
        assert_eq!(
            client.call(
                "thread_start",
                json!({"path": "a.md", "body": "no identity"})
            )?["isError"],
            true
        );
    }
    let mut client = Mcp::start(&fixture, "codex", &[])?;
    for meta in [
        json!({"sessionId": "family"}),
        json!({"threadId": "thread"}),
        json!({"sessionId": "family", "threadId": ""}),
        json!({"sessionId": 42, "threadId": "thread"}),
    ] {
        client.meta = meta;
        assert_eq!(
            client.call("follow", json!({"type": "coder"}))?["isError"],
            true
        );
    }
    let mut client = Mcp::copilot(&fixture, "real")?;
    for (tool, arguments) in [
        ("follow", json!({"id": "forged", "type": "coder"})),
        ("threads", json!({"id": "forged"})),
        (
            "thread_start",
            json!({"id": "forged", "path": "a.md", "body": "forged"}),
        ),
        (
            "thread_reply",
            json!({"id": "forged", "thread": "absent", "body": "forged"}),
        ),
        (
            "thread_watch",
            json!({"id": "forged", "on": "absent", "when": "message"}),
        ),
        ("workspaces", json!({"switch": fixture.root})),
    ] {
        let result = client.call(tool, arguments)?;
        assert_eq!(
            result["isError"], true,
            "{tool} accepted a removed parameter: {result}"
        );
        assert!(result.to_string().contains("unknown field"), "{result}");
    }
    assert!(!fixture.dirs.agents_file(&fixture.root).exists());
    assert!(fixture.store(&fixture.root)?.threads().is_empty());
    Ok(())
}

#[cfg(unix)]
#[test]
fn non_unicode_launch_identity_is_not_used() -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let fixture = Fixture::new("mcp-unicode-identity")?;
    let mut client = Mcp::start(
        &fixture,
        "claude-code",
        &[("CLAUDE_CODE_SESSION_ID", OsStr::from_bytes(b"\xff"))],
    )?;
    client.ok("threads", json!({}))?;
    assert_eq!(
        client.call("follow", json!({"type": "coder"}))?["isError"],
        true
    );
    Ok(())
}

#[test]
fn resumed_launch_recovers_profile_but_corrupt_register_refuses_writes() -> Result<()> {
    let fixture = Fixture::new("mcp-resume-identity")?;
    {
        let mut client = Mcp::copilot(&fixture, "same-chat")?;
        client.ok("follow", json!({"type": "coder", "persona": "Builder"}))?
    };
    let mut client = Mcp::copilot(&fixture, "same-chat")?;
    client.ok("thread_start", json!({"path": "a.md", "body": "resumed"}))?;
    let store = fixture.store(&fixture.root)?;
    assert_eq!(store.threads()[0].author().id(), Some("copilot:same-chat"));
    assert_eq!(store.threads()[0].author().name(), "Builder");
    fs::write(
        fixture.dirs.agents_file(&fixture.root),
        "invalid register\n",
    )?;
    assert_eq!(
        client.call(
            "thread_start",
            json!({"path": "a.md", "body": "do not write"})
        )?["isError"],
        true
    );
    assert_eq!(fixture.store(&fixture.root)?.threads().len(), 1);
    Ok(())
}

#[test]
fn delivery_hook_uses_the_same_chat_key_without_a_hello() -> Result<()> {
    let fixture = Fixture::new("mcp-hook-identity")?;
    let thread = fixture.user_thread()?;
    let mut client = Mcp::copilot(&fixture, "chat")?;
    client.ok("follow", json!({"type": "coder"}))?;
    let mut hook = fixture
        .command()
        .args(["pending", "--hook", "copilot", "--prompt"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    writeln!(
        hook.stdin.take().context("hook stdin")?,
        "{}",
        json!({"sessionId": "chat", "cwd": fixture.root})
    )?;
    let output = hook.wait_with_output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8(output.stdout)?.contains(&thread.to_string()));
    Ok(())
}
