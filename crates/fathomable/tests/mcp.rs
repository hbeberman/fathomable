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

    fn key(&self) -> Result<PathBuf> {
        Ok(Workspace::discover(&self.root)?.key().to_path_buf())
    }

    fn threads_path(&self) -> Result<PathBuf> {
        Ok(self.dirs.threads_file(&self.key()?))
    }

    fn store(&self) -> Result<Store> {
        Ok(Store::open(self.threads_path()?)?)
    }

    fn user_thread(&self, body: &str) -> Result<ThreadId> {
        Ok(Store::open(self.threads_path()?)?.annotate(
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
        let content = result["content"]
            .as_array()
            .context("result text content")?;
        ensure!(
            content.len() == 1,
            "{name}: expected one JSON text fallback"
        );
        let text = content[0]["text"].as_str().context("JSON text fallback")?;
        let fallback: Value = serde_json::from_str(text)?;
        ensure!(
            fallback == result["structuredContent"],
            "{name}: text fallback differs from structured content"
        );
        Ok(result)
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn schema_contains(schema: &Value, key: &str, expected: &Value) -> bool {
    match schema {
        Value::Object(object) => {
            object.get(key) == Some(expected)
                || object
                    .values()
                    .any(|value| schema_contains(value, key, expected))
        }
        Value::Array(values) => values
            .iter()
            .any(|value| schema_contains(value, key, expected)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
    }
}

fn schema_contains_values(schema: &Value, expected: &[&str]) -> bool {
    match schema {
        Value::Object(object) => {
            let direct = object
                .get("enum")
                .and_then(Value::as_array)
                .is_some_and(|values| {
                    expected
                        .iter()
                        .all(|value| values.iter().any(|item| item == value))
                });
            let one_of = object
                .get("oneOf")
                .and_then(Value::as_array)
                .is_some_and(|variants| {
                    expected.iter().all(|value| {
                        variants.iter().any(|variant| {
                            variant.get("const").and_then(Value::as_str) == Some(*value)
                        })
                    })
                });
            direct
                || one_of
                || object
                    .values()
                    .any(|value| schema_contains_values(value, expected))
        }
        Value::Array(values) => values
            .iter()
            .any(|value| schema_contains_values(value, expected)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
    }
}

fn schema_has_property(schema: &Value, property: &str) -> bool {
    match schema {
        Value::Object(object) => {
            object
                .get("properties")
                .and_then(Value::as_object)
                .is_some_and(|properties| properties.contains_key(property))
                || object
                    .values()
                    .any(|value| schema_has_property(value, property))
        }
        Value::Array(values) => values
            .iter()
            .any(|value| schema_has_property(value, property)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
    }
}

fn referenced_definition<'a>(root: &'a Value, schema: &'a Value) -> Result<&'a Value> {
    if schema.get("$ref").is_none() {
        return Ok(schema);
    }
    let reference = schema["$ref"].as_str().context("schema reference")?;
    let name = reference.rsplit('/').next().context("definition name")?;
    root["$defs"]
        .get(name)
        .with_context(|| format!("definition {name}"))
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
    let store_path = fixture.threads_path()?;
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
                ..
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
        assert!(tool["outputSchema"].is_object(), "{name}: {tool}");
    }
    assert!(
        tools
            .iter()
            .find(|tool| tool["name"] == "threads")
            .and_then(|tool| tool["outputSchema"].as_object())
            .is_some()
    );
    Ok(())
}

#[test]
fn schemas_encode_mcp_input_constraints() -> Result<()> {
    let fixture = Fixture::new("mcp-schemas")?;
    let mut client = Mcp::copilot(&fixture, "chat")?;
    let tools = client.request("tools/list", json!({}))?["tools"]
        .as_array()
        .cloned()
        .context("tools")?;
    let schema = |name: &str| {
        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .map(|tool| tool["inputSchema"].clone())
            .with_context(|| format!("schema for {name}"))
    };

    let threads = schema("threads")?;
    assert!(threads["allOf"].is_array());
    assert_eq!(threads["properties"]["ids"]["uniqueItems"], true);
    assert_eq!(threads["properties"]["ids"]["items"]["type"], "string");
    assert!(schema_contains_values(
        &threads,
        &["open", "resolved", "all"]
    ));

    let start = schema("thread_start")?;
    assert_eq!(start["properties"]["comments"]["minItems"], 1);
    let start_item = referenced_definition(&start, &start["properties"]["comments"]["items"])?;
    assert!(schema_contains(
        &start_item["properties"]["line"],
        "minimum",
        &json!(1)
    ));
    assert!(schema_contains(
        &start_item["properties"]["end_line"],
        "minimum",
        &json!(1)
    ));
    assert!(start_item["allOf"].is_array());

    let reply = schema("thread_reply")?;
    assert_eq!(reply["properties"]["replies"]["minItems"], 1);
    let reply_item = referenced_definition(&reply, &reply["properties"]["replies"]["items"])?;
    assert!(schema_contains(
        &reply_item["properties"]["line"],
        "minimum",
        &json!(1)
    ));
    assert!(schema_contains(
        &reply_item["properties"]["end_line"],
        "minimum",
        &json!(1)
    ));
    assert!(reply_item["allOf"].is_array());
    assert!(schema_contains(
        &reply_item["properties"]["propose_resolve"],
        "type",
        &json!("boolean")
    ));
    assert!(reply_item["properties"]["resolve"].is_null());
    assert!(schema_contains(
        &start_item["properties"]["idempotency_key"],
        "minLength",
        &json!(1)
    ));
    assert!(schema_contains(
        &start_item["properties"]["idempotency_key"],
        "maxLength",
        &json!(256)
    ));
    assert!(schema_contains(
        &reply_item["properties"]["idempotency_key"],
        "minLength",
        &json!(1)
    ));
    assert!(schema_contains(
        &reply_item["properties"]["idempotency_key"],
        "maxLength",
        &json!(256)
    ));

    let output = tools
        .iter()
        .find(|tool| tool["name"] == "threads")
        .context("threads output schema")?["outputSchema"]
        .clone();
    for field in ["anchor_range", "location", "author", "replies", "worktree"] {
        assert!(
            schema_has_property(&output, field),
            "output schema is missing {field}: {output}"
        );
    }
    for values in [
        &["anchored", "edited", "detached", "file"][..],
        &["unchanged", "moved", "detached", "file"][..],
        &["open", "resolved"][..],
        &["user", "agent"][..],
    ] {
        assert!(
            schema_contains_values(&output, values),
            "output schema is missing enum {values:?}: {output}"
        );
    }
    Ok(())
}

#[test]
fn ids_filter_schema_and_handler_rules_match() -> Result<()> {
    let fixture = Fixture::new("mcp-selector-parity")?;
    let thread = fixture.user_thread("find this")?;
    let mut client = Mcp::copilot(&fixture, "chat")?;

    assert!(
        !client.call("threads", json!({}))?["isError"]
            .as_bool()
            .unwrap_or(true)
    );
    assert!(
        !client.call("threads", json!({"ids": [thread]}))?["isError"]
            .as_bool()
            .unwrap_or(true)
    );
    assert!(
        !client.call("threads", json!({"ids": [], "status": "open"}))?["isError"]
            .as_bool()
            .unwrap_or(true)
    );
    assert!(
        !client.call("threads", json!({"ids": [], "status": null}))?["isError"]
            .as_bool()
            .unwrap_or(true)
    );
    assert_eq!(
        client.call("threads", json!({"ids": [thread], "status": "open"}))?["isError"],
        true
    );
    assert_eq!(
        client.call("threads", json!({"ids": [thread], "limit": 1}))?["isError"],
        true
    );
    let invalid = client.call("threads", json!({"status": "closed"}))?;
    assert_eq!(invalid["isError"], true);
    assert!(
        invalid["content"][0]["text"]
            .as_str()
            .context("invalid status error")?
            .contains("`status` is `open`, `resolved`, or `all`, not \"closed\"")
    );
    Ok(())
}

#[test]
fn range_inputs_reject_zero_and_reversed_ranges_before_writing() -> Result<()> {
    let fixture = Fixture::new("mcp-range-validation")?;
    let thread = fixture.user_thread("move me")?;
    let mut client = Mcp::copilot(&fixture, "chat")?;

    for arguments in [
        json!({"comments": [{"path": "a.md", "line": 2, "end_line": 1, "body": "bad"}]}),
        json!({"comments": [{"path": "a.md", "line": 0, "body": "bad"}]}),
    ] {
        assert_eq!(client.call("thread_start", arguments)?["isError"], true);
    }
    for arguments in [
        json!({"replies": [{"thread": thread, "line": 2, "end_line": 1, "body": "bad"}]}),
        json!({"replies": [{"thread": thread, "line": 0, "body": "bad"}]}),
    ] {
        assert_eq!(client.call("thread_reply", arguments)?["isError"], true);
    }
    assert!(
        fixture
            .store()?
            .thread(&thread)
            .context("thread")?
            .replies()
            .is_empty()
    );
    assert_eq!(fixture.store()?.threads().len(), 1);
    Ok(())
}

#[test]
fn keyed_mcp_writes_replay_without_duplicate_effects() -> Result<()> {
    let fixture = Fixture::new("mcp-idempotency")?;
    let mut client = Mcp::copilot(&fixture, "chat")?;
    let duplicate = client.call(
        "thread_start",
        json!({
            "comments": [
                {"path": "a.md", "line": 1, "body": "first", "idempotency_key": "same"},
                {"path": "a.md", "line": 2, "body": "second", "idempotency_key": "same"}
            ]
        }),
    )?;
    assert_eq!(duplicate["isError"], true);
    assert!(fixture.store()?.threads().is_empty());

    let start = json!({
        "comments": [{
            "path": "a.md",
            "line": 1,
            "body": "retry this",
            "idempotency_key": "start-once"
        }]
    });
    let first = client.ok("thread_start", start.clone())?;
    let first_id = first["structuredContent"]["threads"][0]["id"].clone();
    let replay = client.ok("thread_start", start)?;
    assert_eq!(replay["structuredContent"]["threads"][0]["id"], first_id);
    let thread = fixture.store()?.threads()[0].id().clone();
    assert_eq!(thread.to_string(), first_id);
    let second = client.ok(
        "thread_start",
        json!({"comments": [{"path": "a.md", "line": 2, "body": "second"}]}),
    )?;
    let second_thread = second["structuredContent"]["threads"][0]["id"]
        .as_str()
        .context("second thread id")?
        .to_owned();
    let duplicate_replies = client.call(
        "thread_reply",
        json!({
            "replies": [
                {"thread": thread, "body": "first", "idempotency_key": "same-reply"},
                {"thread": second_thread, "body": "second", "idempotency_key": "same-reply"}
            ]
        }),
    )?;
    assert_eq!(duplicate_replies["isError"], true);
    assert!(
        fixture
            .store()?
            .thread(&thread)
            .context("first thread")?
            .replies()
            .is_empty()
    );

    let reply_request = json!({
        "replies": [{
            "thread": first_id,
            "body": "retry reply",
            "idempotency_key": "reply-once"
        }]
    });
    let first_reply = client.ok("thread_reply", reply_request.clone())?;
    let replay_reply = client.ok("thread_reply", reply_request.clone())?;
    assert_eq!(
        first_reply["structuredContent"]["threads"][0]["replies"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        replay_reply["structuredContent"]["threads"][0]["replies"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        fixture
            .store()?
            .thread(&thread)
            .context("thread after replay")?
            .replies()
            .len(),
        1
    );
    Ok(())
}

#[test]
fn keyed_retries_bypass_mutated_start_and_reply_validation() -> Result<()> {
    let fixture = Fixture::new("mcp-idempotency-mutations")?;
    let mut client = Mcp::copilot(&fixture, "chat")?;
    let start_request = json!({
        "comments": [{
            "path": "a.md",
            "line": 1,
            "body": "retry after deletion",
            "idempotency_key": "start-mutation"
        }]
    });
    let first = client.ok("thread_start", start_request.clone())?;
    let thread = first["structuredContent"]["threads"][0]["id"]
        .as_str()
        .context("started thread id")?
        .to_owned();
    let id: ThreadId = serde_json::from_value(json!(thread.clone()))?;
    fs::remove_file(fixture.root.join("a.md"))?;

    let replay = client.ok("thread_start", start_request)?;
    assert_eq!(
        replay["structuredContent"]["threads"][0]["id"],
        thread.as_str()
    );
    assert_eq!(
        replay["structuredContent"]["threads"][0]["placement"],
        "detached"
    );
    assert_eq!(fixture.store()?.threads().len(), 1);

    fs::write(fixture.root.join("a.md"), "one\ntwo\n")?;
    let reply_request = json!({
        "replies": [{
            "thread": thread,
            "line": 2,
            "body": "retry after resolution",
            "idempotency_key": "reply-mutation"
        }]
    });
    client.ok("thread_reply", reply_request.clone())?;
    fs::remove_file(fixture.root.join("a.md"))?;
    fixture.store()?.thread(&id).context("thread after reply")?;
    let mut store = fixture.store()?;
    store.resolve(&id, None, now())?;
    drop(store);

    let replay_reply = client.ok("thread_reply", reply_request.clone())?;
    let replayed = &replay_reply["structuredContent"]["threads"][0];
    assert_eq!(replayed["id"], thread);
    assert_eq!(replayed["status"], "resolved");
    assert_eq!(replayed["placement"], "detached");
    assert_eq!(replayed["replies"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        fixture
            .store()?
            .thread(&id)
            .context("thread after reply replay")?
            .replies()
            .len(),
        1
    );
    let mut store = fixture.store()?;
    store.delete(&id, now())?;
    drop(store);
    let deleted = client.call("thread_reply", reply_request)?;
    assert_eq!(deleted["isError"], true);
    assert!(
        deleted["content"][0]["text"]
            .as_str()
            .context("deleted replay error")?
            .contains("deleted")
    );
    Ok(())
}

#[test]
fn keyed_batches_replay_after_restart_and_reject_conflicts_before_writing() -> Result<()> {
    let fixture = Fixture::new("mcp-keyed-batch-restart")?;
    let mut client = Mcp::copilot(&fixture, "chat")?;
    let first_request = json!({
        "path": "./a.md", "line": 1, "body": "first", "idempotency_key": "first"
    });
    let first = client.ok("thread_start", json!({"comments": [first_request]}))?;
    let first_id = first["structuredContent"]["threads"][0]["id"].clone();
    drop(client);

    let mut restarted = Mcp::copilot(&fixture, "chat")?;
    let mixed = restarted.ok(
        "thread_start",
        json!({"comments": [
            {"path": "a.md", "line": 1, "end_line": 1, "body": "first",
             "idempotency_key": "first"},
            {"path": "a.md", "body": "second", "idempotency_key": "second"}
        ]}),
    )?;
    assert_eq!(mixed["structuredContent"]["threads"][0]["id"], first_id);
    assert_eq!(fixture.store()?.threads().len(), 2);

    let before = fs::read(fixture.threads_path()?)?;
    let conflict = restarted.call(
        "thread_start",
        json!({"comments": [
            {"path": "a.md", "body": "must not be written", "idempotency_key": "new"},
            {"path": "a.md", "line": 2, "body": "first", "idempotency_key": "first"}
        ]}),
    )?;
    assert_eq!(conflict["isError"], true);
    assert_eq!(fs::read(fixture.threads_path()?)?, before);

    let reply = json!({"replies": [{
        "thread": first_id, "body": "answer", "propose_resolve": true,
        "idempotency_key": "answer"
    }]});
    restarted.ok("thread_reply", reply.clone())?;
    drop(restarted);
    let before = fs::read(fixture.threads_path()?)?;
    let mut restarted = Mcp::copilot(&fixture, "chat")?;
    restarted.ok("thread_reply", reply.clone())?;
    assert_eq!(fs::read(fixture.threads_path()?)?, before);
    let mut changed = reply;
    changed["replies"][0]["propose_resolve"] = json!(false);
    assert_eq!(restarted.call("thread_reply", changed)?["isError"], true);
    assert_eq!(fs::read(fixture.threads_path()?)?, before);

    let mut other_caller = Mcp::copilot(&fixture, "other-chat")?;
    other_caller.ok("thread_start", json!({"comments": [first_request]}))?;
    assert_eq!(fixture.store()?.threads().len(), 3);
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
    let path = fixture.threads_path()?;
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
    assert_eq!(answered["author"], json!({"kind": "user", "name": "user"}));
    assert_eq!(
        answered["replies"][0]["author"],
        json!({
            "kind": "agent",
            "name": "Earlier agent",
        })
    );
    let structured = result["structuredContent"].to_string();
    assert_eq!(result["content"][0]["text"], structured);
    let fresh = threads
        .iter()
        .find(|thread| thread["id"] == first.to_string())
        .context("fresh thread")?;
    assert_eq!(fresh["placement"], "anchored");
    assert_eq!(fresh["location"], "unchanged");
    assert_eq!(fresh["anchor_range"], json!({"start": 1, "end": 1}));
    assert_eq!(fs::read(path)?, before, "reading changed annotation state");
    Ok(())
}

#[test]
fn placement_and_location_report_detached_and_file_threads() -> Result<()> {
    let fixture = Fixture::new("mcp-placement-states")?;
    let detached = {
        fs::write(fixture.root.join("gone.md"), "gone\n")?;
        let id = Store::open(fixture.threads_path()?)?.annotate(
            Draft::new(
                Author::User,
                Path::new("gone.md"),
                LineRange::new(1, 1),
                "detached",
            ),
            "gone\n",
            now(),
        )?;
        fs::remove_file(fixture.root.join("gone.md"))?;
        id
    };
    let file = Store::open(fixture.threads_path()?)?.annotate(
        Draft::on_file(Author::User, Path::new("a.md"), "file-wide"),
        "one\ntwo\n",
        now(),
    )?;
    let mut client = Mcp::start(&fixture, "unknown-client", &[])?;
    let result = client.ok("threads", json!({"ids": [detached, file]}))?;
    let threads = result["structuredContent"]["threads"]
        .as_array()
        .context("threads")?;
    let detached = threads
        .iter()
        .find(|thread| thread["id"] == detached.to_string())
        .context("detached thread")?;
    assert_eq!(detached["placement"], "detached");
    assert_eq!(detached["location"], "detached");
    assert_eq!(detached["range"], json!({"start": 1, "end": 1}));
    assert_eq!(detached["anchor_range"], json!({"start": 1, "end": 1}));
    let file = threads
        .iter()
        .find(|thread| thread["id"] == file.to_string())
        .context("file thread")?;
    assert_eq!(file["placement"], "file");
    assert_eq!(file["location"], "file");
    assert!(file["range"].is_null());
    assert!(file["anchor_range"].is_null());
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
fn zero_limit_returns_no_threads() -> Result<()> {
    let fixture = Fixture::new("mcp-zero-limit")?;
    fixture.user_thread("not requested")?;
    let mut client = Mcp::start(&fixture, "unknown-client", &[])?;

    let result = client.ok("threads", json!({"limit": 0}))?;

    assert_eq!(result["structuredContent"]["threads"], json!([]));
    assert_eq!(result["structuredContent"]["more"], 1);
    assert!(result["structuredContent"]["next_after"].is_null());
    Ok(())
}

#[test]
fn startup_housekeeping_preserves_placement_without_read_side_effects() -> Result<()> {
    let fixture = Fixture::new("mcp-read-placement")?;
    let original = "one\ntwo\nthree\n";
    fs::write(fixture.root.join("a.md"), original)?;
    let thread = Store::open(fixture.threads_path()?)?.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(2, 2),
            "this line",
        ),
        original,
        1,
    )?;
    let mut seen = fathomable_core::seen::Store::open(&fixture.dirs.seen_dir(&fixture.key()?))?;
    seen.record(Path::new("a.md"), original)?;
    drop(seen);
    fs::write(fixture.root.join("a.md"), "zero\none\nTWO\nthree\n")?;

    let mut client = Mcp::start(&fixture, "unknown-client", &[])?;
    let path = fixture.threads_path()?;
    let after_startup = fs::read(&path)?;
    let result = client.ok("threads", json!({"ids": [thread]}))?;
    let shown = &result["structuredContent"]["threads"][0];
    assert_eq!(shown["range"], json!({"start": 3, "end": 3}));
    assert_eq!(shown["placement"], "edited");
    assert_eq!(shown["anchor_range"], json!({"start": 3, "end": 3}));
    assert_eq!(shown["location"], "unchanged");
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
    let started_result = client.ok(
        "thread_start",
        json!({"comments": [{"path": "a.md", "line": 2, "body": "new finding"}]}),
    )?;
    assert_eq!(
        started_result["structuredContent"]["threads"][0]["author"],
        json!({
            "kind": "agent",
            "name": "Copilot",
            "client": "copilot-cli",
            "id": "copilot:chat",
        })
    );
    assert_eq!(
        started_result["content"][0]["text"],
        started_result["structuredContent"].to_string()
    );
    let replied_result = client.ok(
        "thread_reply",
        json!({"replies": [{"thread": thread, "body": "my proposal", "propose_resolve": true}]}),
    )?;
    assert_eq!(
        replied_result["structuredContent"]["threads"][0]["replies"][0]["author"],
        json!({
            "kind": "agent",
            "name": "Copilot",
            "client": "copilot-cli",
            "id": "copilot:chat",
        })
    );
    assert_eq!(
        replied_result["content"][0]["text"],
        replied_result["structuredContent"].to_string()
    );
    let store = fixture.store()?;
    let started = store.threads().last().context("started")?;
    assert_eq!(started.author().name(), "Copilot");
    assert_eq!(started.author().id(), Some("copilot:chat"));
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
    assert_eq!(start["structuredContent"]["error_code"], "INVALID_BATCH");
    assert_eq!(start["structuredContent"]["issues"][0]["item_index"], 1);
    assert!(
        start["content"][0]["text"]
            .as_str()
            .context("start batch error")?
            .contains("comments[1]")
    );
    assert_eq!(fixture.store()?.threads().len(), 1);

    let reply = client.call(
        "thread_reply",
        json!({"replies": [
            {"thread": thread, "body": "valid"},
            {"thread": "not-a-thread", "body": "invalid"}
        ]}),
    )?;
    assert_eq!(reply["isError"], true);
    assert_eq!(reply["structuredContent"]["error_code"], "INVALID_BATCH");
    assert_eq!(reply["structuredContent"]["issues"][0]["item_index"], 1);
    assert!(
        reply["content"][0]["text"]
            .as_str()
            .context("reply batch error")?
            .contains("replies[1]")
    );
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
    let file_thread = Store::open(fixture.threads_path()?)?.annotate(
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
fn unknown_arguments_are_rejected_without_writes() -> Result<()> {
    let fixture = Fixture::new("mcp-unknown-arguments")?;
    let thread = fixture.user_thread("why?")?;
    let mut client = Mcp::copilot(&fixture, "chat")?;
    for (tool, arguments) in [
        ("threads", json!({"invented": true})),
        (
            "thread_start",
            json!({"comments": [{"path": "a.md", "body": "x"}], "invented": true}),
        ),
        (
            "thread_reply",
            json!({"replies": [{"thread": thread, "body": "x"}], "invented": true}),
        ),
        (
            "thread_start",
            json!({"comments": [{"path": "a.md", "body": "x", "invented": true}]}),
        ),
        (
            "thread_reply",
            json!({"replies": [{"thread": thread, "body": "x", "invented": true}]}),
        ),
        (
            "thread_reply",
            json!({"replies": [{"thread": thread, "body": "x", "resolve": true}]}),
        ),
    ] {
        let result = client.call(tool, arguments)?;
        assert_eq!(result["isError"], true, "{tool}: {result}");
    }
    let store = fixture.store()?;
    assert_eq!(store.threads().len(), 1);
    assert!(
        store
            .thread(&thread)
            .context("thread")?
            .replies()
            .is_empty()
    );
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
    let path = fixture.threads_path()?;
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
    fathomable_core::seen::Store::open(&fixture.dirs.seen_dir(&fixture.key()?))?
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
fn edited_content_can_remain_at_the_anchored_range() -> Result<()> {
    let fixture = Fixture::new("mcp-edited-unchanged")?;
    let original = "one\ntwo\nthree\n";
    fs::write(fixture.root.join("a.md"), original)?;
    let id = Store::open(fixture.threads_path()?)?.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(2, 2),
            "this line",
        ),
        original,
        now(),
    )?;
    let mut client = Mcp::start(&fixture, "unknown-client", &[])?;
    fs::write(fixture.root.join("a.md"), "one\nTWO\nthree\n")?;

    let result = client.ok("threads", json!({"ids": [id]}))?;
    let shown = &result["structuredContent"]["threads"][0];
    assert_eq!(shown["range"], json!({"start": 2, "end": 2}));
    assert_eq!(shown["placement"], "edited");
    assert_eq!(shown["location"], "unchanged");
    assert_eq!(shown["anchor_range"], json!({"start": 2, "end": 2}));
    Ok(())
}

#[test]
fn replying_at_the_current_multiline_range_preserves_the_anchor() -> Result<()> {
    let fixture = Fixture::new("mcp-current-reply-range")?;
    let id = Store::open(fixture.threads_path()?)?.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 2),
            "these lines",
        ),
        "one\ntwo\n",
        now(),
    )?;
    let mut client = Mcp::copilot(&fixture, "chat")?;

    let result = client.ok(
        "thread_reply",
        json!({"replies": [{
            "thread": id,
            "body": "still on these lines",
            "line": 1,
            "end_line": 2
        }]}),
    )?;

    let shown = &result["structuredContent"]["threads"][0];
    assert_eq!(shown["placement"], "anchored");
    assert_eq!(shown["range"], json!({"start": 1, "end": 2}));
    assert_eq!(shown["anchor_range"], json!({"start": 1, "end": 2}));
    assert!(shown["edited"].is_null());
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
    let reply = json!({"replies": [{
        "thread": id,
        "body": "updated on the feature branch",
        "line": 2,
        "idempotency_key": "feature-reply"
    }]});
    let first_reply = client.ok("thread_reply", reply.clone())?;
    let before_replay = fs::read(fixture.threads_path()?)?;
    let repeated = client.ok("thread_reply", reply)?;
    assert_eq!(
        repeated["structuredContent"], first_reply["structuredContent"],
        "replay changed the sibling worktree placement"
    );
    assert_eq!(
        repeated["structuredContent"]["threads"][0]["worktree"],
        linked.to_string_lossy().as_ref()
    );
    assert_eq!(fs::read(fixture.threads_path()?)?, before_replay);
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
fn review_startup_uses_fresh_git_common_dir_state() -> Result<()> {
    let fixture = Fixture::new("mcp-common-dir-state")?;
    fathomable_testing::git::init(&fixture.root)?;
    fathomable_testing::git::commit_and_stage(&fixture.root, &[("a.md", "one\ntwo\n")])?;
    let workspace = Workspace::discover(&fixture.root)?;
    ensure!(
        workspace.key() != fixture.root,
        "Git workspace key should not be root"
    );
    assert!(!fixture.dirs.threads_file(workspace.key()).exists());
    assert!(!fixture.dirs.threads_file(&fixture.root).exists());

    let mut client = Mcp::copilot(&fixture, "chat")?;
    let result = client.ok(
        "thread_start",
        json!({"comments": [{"path": "a.md", "line": 1, "body": "fresh review"}]}),
    )?;
    let id = result["structuredContent"]["threads"][0]["id"]
        .as_str()
        .context("started thread id")?;
    let store = fixture.store()?;
    let result = client.ok("threads", json!({}))?;
    assert_eq!(
        result["structuredContent"]["threads"][0]["id"], id,
        "startup did not use the fresh common-dir store"
    );
    assert_eq!(store.threads().len(), 1);
    assert!(!fixture.dirs.threads_file(&fixture.root).exists());
    Ok(())
}
