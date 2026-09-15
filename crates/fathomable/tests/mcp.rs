//! Session identity and workspace isolation at the stdio MCP boundary.

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
use fathomable_core::annotations::{Author, Draft, LineRange, Store};
use fathomable_core::clock::now;
use fathomable_core::config::AgentsConfig;
use fathomable_core::session::Marker;
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

    fn register(&self, root: &Path) -> Result<Register> {
        Ok(Register::open(
            self.dirs.agents_file(root),
            now(),
            AgentsConfig::default().expire_after,
        )?)
    }
}

struct Mcp {
    child: Child,
    input: ChildStdin,
    output: Receiver<std::io::Result<String>>,
    sequence: u64,
}

impl Mcp {
    fn start(fixture: &Fixture, session: Option<&OsStr>) -> Result<Self> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fathomable"));
        command
            .arg("--mcp")
            .current_dir(&fixture.root)
            .env("HOME", &fixture.dir.0)
            .env("XDG_CONFIG_HOME", fixture.dir.0.join("config"))
            .env("XDG_STATE_HOME", fixture.dir.0.join("state"))
            .env_remove("XDG_RUNTIME_DIR")
            .env_remove("COPILOT_AGENT_SESSION_ID")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if let Some(session) = session {
            command.env("COPILOT_AGENT_SESSION_ID", session);
        }
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
        let mut client = Self {
            child,
            input,
            output,
            sequence: 0,
        };
        client.request(
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "copilot", "version": "test"}
            }),
        )?;
        writeln!(
            client.input,
            "{}",
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
        )?;
        Ok(client)
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
        let mut params = json!({"name": name});
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
fn copilot_identity_is_opt_in_and_drives_all_subscription_tools() -> Result<()> {
    let fixture = Fixture::new("mcp-env-tools")?;
    let mut store = Store::open(fixture.dirs.threads_file(&fixture.root))?;
    let thread = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "why?",
        ),
        "one\ntwo\n",
        now(),
    )?;
    let mut client = Mcp::start(&fixture, Some(OsStr::new("copilot-session")))?;
    client.ok("workspaces", json!({}))?;
    client.ok("threads", json!({"status": "pending"}))?;
    assert!(!fixture.dirs.agents_file(&fixture.root).exists());

    let followed = client.ok("follow", json!({"type": "coder"}))?;
    assert!(followed.to_string().contains("copilot-session"));
    assert!(
        followed
            .to_string()
            .contains(&fixture.root.display().to_string())
    );
    let register = fixture.register(&fixture.root)?;
    assert!(register.bonds().is_empty());
    assert!(register.subscriber("copilot-session").is_some());
    client.ok("follow", json!({"type": "coder"}))?;
    client.ok("threads", json!({"status": "pending"}))?;
    let register = fixture.register(&fixture.root)?;
    let subscriber = register
        .subscriber("copilot-session")
        .context("subscriber")?;
    assert!(
        register
            .deliverable(subscriber, store.threads().iter())
            .is_empty()
    );

    client.ok("thread_watch", json!({"on": thread, "when": "message"}))?;
    assert_eq!(fixture.register(&fixture.root)?.watches().len(), 1);
    client.ok("thread_watch", json!({"on": thread, "cancel": true}))?;
    assert!(fixture.register(&fixture.root)?.watches().is_empty());
    client.ok("thread_reply", json!({"thread": thread, "body": "because"}))?;
    let store = Store::open(fixture.dirs.threads_file(&fixture.root))?;
    let reply = &store.thread(&thread).context("thread")?.replies()[0];
    assert_eq!(reply.author().id(), Some("copilot-session"));

    client.ok("follow", json!({"end": true}))?;
    assert!(fixture.register(&fixture.root)?.subscribers().is_empty());
    assert_eq!(client.call("follow", json!({}))?["isError"], true);
    Ok(())
}

#[test]
fn explicit_identity_overrides_environment_and_connection_memory() -> Result<()> {
    let fixture = Fixture::new("mcp-env-explicit")?;
    let mut client = Mcp::start(&fixture, Some(OsStr::new("launch-session")))?;
    client.ok("follow", json!({"type": "coder"}))?;
    client.ok(
        "follow",
        json!({"id": "explicit-session", "type": "reviewer"}),
    )?;
    client.ok("follow", json!({"type": "reviewer"}))?;
    client.ok(
        "thread_start",
        json!({"path": "a.md", "body": "remembered"}),
    )?;
    client.ok(
        "thread_start",
        json!({"path": "a.md", "body": "explicit", "id": "launch-session"}),
    )?;
    let store = Store::open(fixture.dirs.threads_file(&fixture.root))?;
    assert_eq!(store.threads()[0].author().id(), Some("explicit-session"));
    assert_eq!(store.threads()[1].author().id(), Some("launch-session"));
    client.ok("follow", json!({"end": true}))?;
    let register = fixture.register(&fixture.root)?;
    assert!(register.subscriber("explicit-session").is_none());
    assert!(register.subscriber("launch-session").is_some());
    Ok(())
}

#[test]
fn restart_recovers_identity_but_never_another_workspaces_subscription() -> Result<()> {
    let fixture = Fixture::new("mcp-env-workspaces")?;
    let other = fixture.dir.0.join("other");
    fs::create_dir(&other)?;
    fs::write(other.join("a.md"), "other\n")?;
    Marker::new(other.clone(), vec![other.clone()]).write(&fixture.dirs)?;
    {
        let mut client = Mcp::start(&fixture, Some(OsStr::new("copilot-session")))?;
        client.ok("follow", json!({"type": "coder"}))?
    };
    let mut client = Mcp::start(&fixture, Some(OsStr::new("copilot-session")))?;
    client.ok("thread_start", json!({"path": "a.md", "body": "resumed"}))?;
    let store = Store::open(fixture.dirs.threads_file(&fixture.root))?;
    assert_eq!(store.threads()[0].author().id(), Some("copilot-session"));
    client.ok("follow", json!({"type": "coder"}))?;
    client.ok("workspaces", json!({"switch": other}))?;
    client.ok(
        "thread_start",
        json!({"path": "a.md", "body": "not subscribed here"}),
    )?;
    assert_eq!(client.call("follow", json!({}))?["isError"], true);
    assert!(!fixture.dirs.agents_file(&other).exists());
    let store = Store::open(fixture.dirs.threads_file(&other))?;
    assert_eq!(store.threads()[0].author().id(), None);
    client.ok("follow", json!({"type": "reviewer"}))?;
    client.ok("follow", json!({"end": true}))?;
    assert!(fixture.register(&other)?.subscribers().is_empty());
    assert!(
        fixture
            .register(&fixture.root)?
            .subscriber("copilot-session")
            .is_some()
    );
    client.ok(
        "thread_start",
        json!({"workspace": fixture.root, "path": "a.md", "body": "explicit workspace"}),
    )?;
    let store = Store::open(fixture.dirs.threads_file(&fixture.root))?;
    assert_eq!(store.threads()[1].author().id(), Some("copilot-session"));
    Ok(())
}

#[test]
fn missing_or_blank_environment_preserves_manual_access() -> Result<()> {
    let fixture = Fixture::new("mcp-env-missing")?;
    for session in [None, Some(OsStr::new("")), Some(OsStr::new(" \t"))] {
        let mut client = Mcp::start(&fixture, session)?;
        let result = client.call("follow", json!({"type": "coder"}))?;
        assert_eq!(result["isError"], true);
        assert!(result.to_string().contains("Do not ask the user"));
        client.ok("threads", json!({}))?;
        client.ok("follow", json!({"id": "manual", "type": "coder"}))?;
        client.ok("follow", json!({"end": true}))?;
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn non_unicode_environment_preserves_manual_access() -> Result<()> {
    use std::os::unix::ffi::OsStrExt;

    let fixture = Fixture::new("mcp-env-unicode")?;
    let mut client = Mcp::start(&fixture, Some(OsStr::from_bytes(b"\xff")))?;
    assert_eq!(
        client.call("follow", json!({"type": "coder"}))?["isError"],
        true
    );
    assert!(!fixture.dirs.agents_file(&fixture.root).exists());
    client.ok("follow", json!({"id": "manual", "type": "coder"}))?;
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn environment_precedes_bonds_and_bonds_remain_the_fallback() -> Result<()> {
    let fixture = Fixture::new("mcp-env-bonds")?;
    let ancestors = fathomable_core::bond::ancestors();
    ensure!(!ancestors.is_empty(), "test process needs a /proc ancestor");
    fixture
        .register(&fixture.root)?
        .bond("hook-session", ancestors, now())?;
    {
        let mut client = Mcp::start(&fixture, Some(OsStr::new("launch-session")))?;
        client.ok("follow", json!({"type": "coder"}))?
    };
    let register = fixture.register(&fixture.root)?;
    assert!(register.subscriber("launch-session").is_some());
    assert!(register.subscriber("hook-session").is_none());
    let mut client = Mcp::start(&fixture, None)?;
    client.ok("follow", json!({"type": "reviewer"}))?;
    assert!(
        fixture
            .register(&fixture.root)?
            .subscriber("hook-session")
            .is_some()
    );
    Ok(())
}

#[test]
fn unreadable_subscription_state_refuses_writes_instead_of_guessing_a_signature() -> Result<()> {
    let fixture = Fixture::new("mcp-env-broken-register")?;
    let mut client = Mcp::start(&fixture, Some(OsStr::new("copilot-session")))?;
    client.ok("follow", json!({"type": "coder"}))?;
    fs::write(
        fixture.dirs.agents_file(&fixture.root),
        "invalid register\n",
    )?;
    let response = client.call(
        "thread_start",
        json!({"path": "a.md", "body": "do not write"}),
    )?;
    assert_eq!(response["isError"], true);
    assert!(!fixture.dirs.threads_file(&fixture.root).exists());
    Ok(())
}
