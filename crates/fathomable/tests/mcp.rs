//! Behavior-level tests for the repository-bound MCP review contract.

use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use fathomable_core::XdgDirs;
use fathomable_core::annotations::{
    Author, AutoResolve, Draft, Lifecycle, LineRange, MessageTarget, Reply, Status, Store,
    ThreadId, UserSubmit,
};
use fathomable_core::clock::now;
use fathomable_core::session::{Id, Record};
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

fn assert_mcp_output_schemas(tools: &[Value]) -> Result<()> {
    let output = tools
        .iter()
        .find(|tool| tool["name"] == "threads")
        .context("threads output schema")?["outputSchema"]
        .clone();
    for field in [
        "checkout",
        "resolved_commit",
        "origin",
        "placement_evidence",
        "anchor_range",
        "location",
        "lifecycle",
        "modified",
        "auto_resolve",
        "archived",
        "messages",
        "resolution_history",
        "archive_history",
        "restore_history",
        "reanchored_at",
        "worktree",
    ] {
        assert!(
            schema_has_property(&output, field),
            "output schema is missing {field}: {output}"
        );
    }
    assert!(schema_contains(
        &output,
        "pattern",
        &json!("^[0-9a-f]{40}$")
    ));
    let start_output = tools
        .iter()
        .find(|tool| tool["name"] == "thread_start")
        .context("thread_start output schema")?["outputSchema"]
        .clone();
    assert!(schema_has_property(&start_output, "resolved_commit"));
    assert!(schema_contains(
        &start_output,
        "pattern",
        &json!("^[0-9a-f]{40}$")
    ));
    for values in [
        &["anchored", "edited", "detached", "file"][..],
        &["unchanged", "moved", "detached", "file"][..],
        &["open", "resolved"][..],
        &["active", "resolution_proposed", "resolved"][..],
        &["user", "agent"][..],
    ] {
        assert!(
            schema_contains_values(&output, values),
            "output schema is missing enum {values:?}: {output}"
        );
    }
    let reply_output = tools
        .iter()
        .find(|tool| tool["name"] == "thread_reply")
        .context("thread_reply output schema")?["outputSchema"]
        .clone();
    for field in ["results", "thread", "resolution", "outcome", "replayed"] {
        assert!(
            schema_has_property(&reply_output, field),
            "reply output schema is missing {field}: {reply_output}"
        );
    }
    assert!(schema_contains_values(
        &reply_output,
        &["not_requested", "resolution_proposed", "resolved"]
    ));
    assert!(schema_contains_values(
        &reply_output,
        &["pending_fathomable_user_review"]
    ));
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
        let expected_len = if name == "thread_start" { 2 } else { 1 };
        assert_eq!(properties.len(), expected_len, "{name}: {properties:?}");
        assert!(properties.contains_key(required));
        assert_eq!(
            properties.contains_key("source"),
            name == "thread_start",
            "{name}: {properties:?}"
        );
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
    assert!(schema_has_property(&threads, "source"));

    let start = schema("thread_start")?;
    assert!(schema_has_property(&start, "source"));
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
    assert!(schema_contains(start_item, "maxLength", &json!(1024)));
    assert!(
        start_item["properties"]["body"]["description"]
            .as_str()
            .is_some_and(|description| {
                description.contains("PR-style review comment")
                    && description.contains("Substantial replacements belong in the worktree")
            })
    );

    let reply = schema("thread_reply")?;
    assert!(!schema_has_property(&reply, "source"));
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
    assert!(schema_contains(reply_item, "maxLength", &json!(1024)));
    assert!(
        reply_item["properties"]["body"]["description"]
            .as_str()
            .is_some_and(|description| {
                description.contains("PR-style review reply")
                    && description.contains("small focused snippet")
            })
    );
    assert!(schema_contains(
        &reply_item["properties"]["resolve"],
        "type",
        &json!("boolean")
    ));
    assert!(reply_item["properties"]["propose_resolve"].is_null());
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

    assert_mcp_output_schemas(&tools)?;
    Ok(())
}

#[test]
fn commit_source_captures_historical_text_and_echoes_full_id() -> Result<()> {
    let fixture = Fixture::new("mcp-commit-source-capture")?;
    fathomable_testing::git::init(&fixture.root)?;
    fathomable_testing::git::commit_and_stage(
        &fixture.root,
        &[("a.md", "historical\nsecond\n"), ("old.md", "gone\n")],
    )?;
    let commit = Workspace::discover(&fixture.root)?
        .head_commit()
        .context("HEAD")?;
    fs::write(fixture.root.join("a.md"), "working\n")?;

    let mut client = Mcp::copilot(&fixture, "commit-source-capture")?;
    let selected = client.ok(
        "thread_start",
        json!({
            "source": {"kind": "commit", "revision": "HEAD"},
            "comments": [
                {"path": "a.md", "line": 1, "body": "historical line"},
                {"path": "old.md", "line": 1, "body": "historical-only path"}
            ]
        }),
    )?;
    assert_eq!(selected["structuredContent"]["resolved_commit"], commit);
    assert_eq!(
        selected["structuredContent"]["threads"][0]["origin"]["snippet"],
        "historical"
    );
    assert_eq!(
        selected["structuredContent"]["threads"][0]["origin"]["version"],
        json!({"kind": "commit", "id": commit})
    );
    assert_eq!(
        selected["structuredContent"]["threads"][1]["placement"],
        "detached"
    );

    let ordinary = client.ok("threads", json!({"source": null}))?;
    assert!(
        ordinary["structuredContent"]
            .get("resolved_commit")
            .is_none()
    );
    Ok(())
}

#[test]
fn commit_source_reads_exact_origins_and_uses_origin_paths() -> Result<()> {
    let fixture = Fixture::new("mcp-commit-source-filter")?;
    fathomable_testing::git::init(&fixture.root)?;
    fathomable_testing::git::commit_and_stage(
        &fixture.root,
        &[("a.md", "one\ntwo\n"), ("historical/only.md", "old\n")],
    )?;
    let commit = Workspace::discover(&fixture.root)?
        .head_commit()
        .context("HEAD")?;
    let mut client = Mcp::copilot(&fixture, "commit-source-filter")?;
    client.ok(
        "thread_start",
        json!({
            "source": {"kind": "commit", "revision": commit},
            "comments": [{"path": "historical/only.md", "line": 1, "body": "selected"}]
        }),
    )?;
    client.ok(
        "thread_start",
        json!({"comments": [{"path": "a.md", "line": 1, "body": "working tree"}]}),
    )?;

    let selected = client.ok(
        "threads",
        json!({
            "source": {"kind": "commit", "revision": commit},
            "path": "historical"
        }),
    )?;
    assert_eq!(selected["structuredContent"]["resolved_commit"], commit);
    assert_eq!(
        selected["structuredContent"]["threads"]
            .as_array()
            .context("selected threads")?
            .len(),
        1
    );
    assert_eq!(
        selected["structuredContent"]["threads"][0]["origin"]["path"],
        "historical/only.md"
    );
    let empty = client.ok(
        "threads",
        json!({
            "source": {"kind": "commit", "revision": commit.to_uppercase()},
            "path": "missing",
            "limit": 0,
            "ids": []
        }),
    )?;
    assert_eq!(empty["structuredContent"]["resolved_commit"], commit);
    assert_eq!(empty["structuredContent"]["threads"], json!([]));
    assert!(empty["structuredContent"]["next_after"].is_null());
    Ok(())
}

#[test]
fn commit_source_pagination_is_pinned_and_rejects_cross_mode_cursors() -> Result<()> {
    let fixture = Fixture::new("mcp-commit-source-page")?;
    fathomable_testing::git::init(&fixture.root)?;
    fathomable_testing::git::commit_and_stage(&fixture.root, &[("a.md", "one\ntwo\n")])?;
    let commit = Workspace::discover(&fixture.root)?
        .head_commit()
        .context("HEAD")?;
    let mut client = Mcp::copilot(&fixture, "commit-source-page")?;
    client.ok(
        "thread_start",
        json!({
            "source": {"kind": "commit", "revision": commit},
            "comments": [
                {"path": "a.md", "line": 1, "body": "first"},
                {"path": "a.md", "line": 2, "body": "second"}
            ]
        }),
    )?;
    let first = client.ok(
        "threads",
        json!({"source": {"kind": "commit", "revision": "HEAD"}, "limit": 1}),
    )?;
    let after = first["structuredContent"]["next_after"].clone();
    assert_eq!(after["resolved_commit"], commit);

    fathomable_testing::git::commit_and_stage(&fixture.root, &[("a.md", "changed\n")])?;
    let moved = Workspace::discover(&fixture.root)?
        .head_commit()
        .context("moved HEAD")?;
    assert_eq!(
        client.call(
            "threads",
            json!({
                "source": {"kind": "commit", "revision": "HEAD"},
                "after": after
            }),
        )?["isError"],
        true
    );
    let second = client.ok(
        "threads",
        json!({
            "source": {"kind": "commit", "revision": commit},
            "after": after,
            "limit": 1
        }),
    )?;
    assert_eq!(second["structuredContent"]["resolved_commit"], commit);
    assert_eq!(
        second["structuredContent"]["threads"]
            .as_array()
            .context("continued selected threads")?
            .len(),
        1
    );

    assert_eq!(
        client.call("threads", json!({"after": after}))?["isError"],
        true
    );
    let board_after = json!({"updated": after["updated"], "id": after["id"]});
    assert_eq!(
        client.call(
            "threads",
            json!({
                "source": {"kind": "commit", "revision": commit},
                "after": board_after
            }),
        )?["isError"],
        true
    );
    let mismatched = json!({
        "updated": after["updated"],
        "id": after["id"],
        "resolved_commit": moved
    });
    assert_eq!(
        client.call(
            "threads",
            json!({
                "source": {"kind": "commit", "revision": commit},
                "after": mismatched
            }),
        )?["isError"],
        true
    );
    Ok(())
}

#[test]
fn commit_source_idempotency_distinguishes_sources_and_modes() -> Result<()> {
    let fixture = Fixture::new("mcp-commit-source-retry")?;
    fathomable_testing::git::init(&fixture.root)?;
    fathomable_testing::git::commit_and_stage(&fixture.root, &[("a.md", "one\n")])?;
    let first_commit = Workspace::discover(&fixture.root)?
        .head_commit()
        .context("HEAD")?;
    let mut client = Mcp::copilot(&fixture, "commit-source-retry")?;
    let request = json!({
        "source": {"kind": "commit", "revision": first_commit},
        "comments": [{
            "path": "a.md",
            "line": 1,
            "body": "keyed",
            "idempotency_key": "commit-source-key"
        }]
    });
    let first = client.ok("thread_start", request.clone())?;
    let replay = client.ok("thread_start", request)?;
    assert_eq!(
        first["structuredContent"]["threads"][0]["id"],
        replay["structuredContent"]["threads"][0]["id"]
    );

    fathomable_testing::git::commit_and_stage(&fixture.root, &[("a.md", "two\n")])?;
    let second_commit = Workspace::discover(&fixture.root)?
        .head_commit()
        .context("HEAD")?;
    assert_ne!(first_commit, second_commit);
    assert_eq!(
        client.call(
            "thread_start",
            json!({
                "source": {"kind": "commit", "revision": second_commit},
                "comments": [{
                    "path": "a.md", "line": 1, "body": "keyed",
                    "idempotency_key": "commit-source-key"
                }]
            }),
        )?["isError"],
        true
    );
    assert_eq!(
        client.call(
            "thread_start",
            json!({
                "comments": [{
                    "path": "a.md", "line": 1, "body": "keyed",
                    "idempotency_key": "commit-source-key"
                }]
            }),
        )?["isError"],
        true
    );
    Ok(())
}

#[test]
fn commit_source_full_id_replay_does_not_reload_collected_object() -> Result<()> {
    let fixture = Fixture::new("mcp-commit-source-collected-replay")?;
    fathomable_testing::git::init(&fixture.root)?;
    fathomable_testing::git::commit_and_stage(&fixture.root, &[("a.md", "one\n")])?;
    let commit = Workspace::discover(&fixture.root)?
        .head_commit()
        .context("HEAD")?;
    let mut client = Mcp::copilot(&fixture, "commit-source-collected-replay")?;
    let request = json!({
        "source": {"kind": "commit", "revision": commit},
        "comments": [{
            "path": "a.md",
            "line": 1,
            "body": "keyed",
            "idempotency_key": "collected-object-key"
        }]
    });
    let first = client.ok("thread_start", request.clone())?;
    let id = first["structuredContent"]["threads"][0]["id"].clone();

    let object = fixture
        .root
        .join(".git/objects")
        .join(&commit[..2])
        .join(&commit[2..]);
    fs::remove_file(object)?;
    let replay = client.ok("thread_start", request)?;
    assert_eq!(replay["structuredContent"]["resolved_commit"], commit);
    assert_eq!(replay["structuredContent"]["threads"][0]["id"], id);
    Ok(())
}

#[test]
fn commit_source_rejects_invalid_selectors_and_source_failures() -> Result<()> {
    let fixture = Fixture::new("mcp-commit-source-invalid")?;
    fathomable_testing::git::init(&fixture.root)?;
    fathomable_testing::git::commit_and_stage(
        &fixture.root,
        &[("a.md", "one\n"), ("binary.dat", "a\0b")],
    )?;
    let commit = Workspace::discover(&fixture.root)?
        .head_commit()
        .context("HEAD")?;
    let mut client = Mcp::copilot(&fixture, "commit-source-invalid")?;
    for source in [
        json!({"kind": "commit", "revision": " HEAD"}),
        json!({"kind": "commit", "revision": "HEAD~1"}),
        json!({"kind": "tag", "revision": "HEAD"}),
        json!({"kind": "commit", "revision": "HEAD", "extra": true}),
    ] {
        assert_eq!(
            client.call("threads", json!({"source": source}))?["isError"],
            true
        );
    }

    assert_eq!(
        client.call(
            "threads",
            json!({"source": {"kind": "commit", "revision": commit}, "ids": ["1-1-1"]}),
        )?["isError"],
        true
    );
    let bad = client.call(
        "thread_start",
        json!({
            "source": {"kind": "commit", "revision": commit},
            "comments": [
                {"path": "missing.md", "line": 1, "body": "missing"},
                {"path": "binary.dat", "line": 1, "body": "binary"}
            ]
        }),
    )?;
    assert_eq!(bad["isError"], true);
    assert_eq!(bad["structuredContent"]["error_code"], "INVALID_BATCH");
    assert_eq!(
        bad["structuredContent"]["issues"]
            .as_array()
            .context("selected batch issues")?
            .len(),
        2
    );
    Ok(())
}

#[test]
fn commit_source_rejects_invalid_utf8_and_git_symlinks() -> Result<()> {
    let fixture = Fixture::new("mcp-commit-source-object-kinds")?;
    fathomable_testing::git::init(&fixture.root)?;
    fathomable_testing::git::commit_bytes_and_stage(
        &fixture.root,
        "invalid.dat",
        &[0xff, 0xfe, b'\n'],
    )?;
    let invalid_commit = Workspace::discover(&fixture.root)?
        .head_commit()
        .context("invalid UTF-8 commit")?;
    fathomable_testing::git::commit_symlink_and_stage(&fixture.root, "link.md", "a.md")?;
    let symlink_commit = Workspace::discover(&fixture.root)?
        .head_commit()
        .context("symlink commit")?;
    let mut client = Mcp::copilot(&fixture, "commit-source-object-kinds")?;

    for (commit, path) in [(invalid_commit, "invalid.dat"), (symlink_commit, "link.md")] {
        let result = client.call(
            "thread_start",
            json!({
                "source": {"kind": "commit", "revision": commit},
                "comments": [{"path": path, "line": 1, "body": "unsupported source"}]
            }),
        )?;
        assert_eq!(result["isError"], true);
        assert_eq!(result["structuredContent"]["error_code"], "INVALID_BATCH");
        assert_eq!(result["structuredContent"]["issues"][0]["item_index"], 0);
    }
    assert!(fixture.store()?.threads().is_empty());
    Ok(())
}

#[test]
fn commit_source_caches_duplicate_paths_and_enforces_cumulative_budget() -> Result<()> {
    const MIB: usize = 1024 * 1024;

    let fixture = Fixture::new("mcp-commit-source-budget")?;
    fathomable_testing::git::init(&fixture.root)?;
    let large = "x".repeat(40 * MIB);
    let other = "y".repeat(25 * MIB);
    fathomable_testing::git::commit_and_stage(
        &fixture.root,
        &[("large.md", &large), ("other.md", &other)],
    )?;
    let commit = Workspace::discover(&fixture.root)?
        .head_commit()
        .context("large commit")?;
    let mut client = Mcp::copilot(&fixture, "commit-source-budget")?;

    let duplicate = client.ok(
        "thread_start",
        json!({
            "source": {"kind": "commit", "revision": commit},
            "comments": [
                {"path": "large.md", "line": 1, "body": "first use"},
                {"path": "large.md", "line": 1, "body": "cached use"}
            ]
        }),
    )?;
    assert_eq!(
        duplicate["structuredContent"]["threads"]
            .as_array()
            .context("duplicate-path starts")?
            .len(),
        2
    );

    let over_budget = client.call(
        "thread_start",
        json!({
            "source": {"kind": "commit", "revision": commit},
            "comments": [
                {"path": "large.md", "line": 1, "body": "large"},
                {"path": "other.md", "line": 1, "body": "cumulative"}
            ]
        }),
    )?;
    assert_eq!(over_budget["isError"], true);
    assert_eq!(
        over_budget["structuredContent"]["error_code"],
        "INVALID_BATCH"
    );
    assert_eq!(fixture.store()?.threads().len(), 2);
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
        first_reply["structuredContent"]["results"][0]["thread"]["messages"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(
        replay_reply["structuredContent"]["results"][0]["thread"]["messages"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(
        first_reply["structuredContent"]["results"][0]["replayed"],
        false
    );
    assert_eq!(
        replay_reply["structuredContent"]["results"][0]["replayed"],
        true
    );
    assert_eq!(
        replay_reply["structuredContent"]["results"][0]["resolution"]["outcome"],
        "not_requested"
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
fn keyed_legacy_oversized_messages_still_replay() -> Result<()> {
    let fixture = Fixture::new("mcp-legacy-large-replay")?;
    let mut client = Mcp::copilot(&fixture, "chat")?;

    let start_exact = "é".repeat(512);
    let start_large = format!("{start_exact}é");
    let start_key = "legacy-large-start";
    let first = client.ok(
        "thread_start",
        json!({"comments": [{
            "path": "a.md",
            "line": 1,
            "body": start_exact,
            "idempotency_key": start_key
        }]}),
    )?;
    let first_id = first["structuredContent"]["threads"][0]["id"].clone();
    let path = fixture.threads_path()?;
    let persisted = fs::read_to_string(&path)?;
    fs::write(&path, persisted.replace(&start_exact, &start_large))?;

    let replay = client.ok(
        "thread_start",
        json!({"comments": [{
            "path": "a.md",
            "line": 1,
            "body": start_large,
            "idempotency_key": start_key
        }]}),
    )?;
    assert_eq!(replay["structuredContent"]["threads"][0]["id"], first_id);
    assert_eq!(
        replay["structuredContent"]["threads"][0]["messages"][0]["body"],
        start_large
    );

    let reply_thread = fixture.user_thread("reply target")?;
    let reply_exact = "ü".repeat(512);
    let reply_large = format!("{reply_exact}ü");
    let reply_key = "legacy-large-reply";
    client.ok(
        "thread_reply",
        json!({"replies": [{
            "thread": reply_thread,
            "body": reply_exact,
            "idempotency_key": reply_key
        }]}),
    )?;
    let persisted = fs::read_to_string(&path)?;
    fs::write(&path, persisted.replace(&reply_exact, &reply_large))?;

    let replay = client.ok(
        "thread_reply",
        json!({"replies": [{
            "thread": reply_thread,
            "body": reply_large,
            "idempotency_key": reply_key
        }]}),
    )?;
    assert_eq!(replay["structuredContent"]["results"][0]["replayed"], true);
    assert_eq!(
        replay["structuredContent"]["results"][0]["thread"]["messages"][1]["body"],
        reply_large
    );
    assert_eq!(
        fixture
            .store()?
            .thread(&reply_thread)
            .context("reply thread")?
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
    let replayed = &replay_reply["structuredContent"]["results"][0]["thread"];
    assert_eq!(replayed["id"], thread);
    assert_eq!(replayed["status"], "resolved");
    assert_eq!(replayed["placement"], "detached");
    assert_eq!(replayed["messages"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        replay_reply["structuredContent"]["results"][0]["resolution"]["outcome"],
        "not_requested"
    );
    assert_eq!(
        replay_reply["structuredContent"]["results"][0]["replayed"],
        true
    );
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
        "thread": first_id, "body": "answer", "resolve": true,
        "idempotency_key": "answer"
    }]});
    restarted.ok("thread_reply", reply.clone())?;
    drop(restarted);
    let before = fs::read(fixture.threads_path()?)?;
    let mut restarted = Mcp::copilot(&fixture, "chat")?;
    restarted.ok("thread_reply", reply.clone())?;
    assert_eq!(fs::read(fixture.threads_path()?)?, before);
    let mut changed = reply;
    changed["replies"][0]["resolve"] = json!(false);
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
    assert_eq!(answered["messages"][0]["body"], "another question");
    assert_eq!(answered["messages"][1]["body"], "my proposal");
    assert_eq!(
        answered["messages"][0]["author"],
        json!({"kind": "user", "name": "user"})
    );
    assert_eq!(
        answered["messages"][1]["author"],
        json!({
            "kind": "agent",
            "name": "Earlier agent",
        })
    );
    assert_eq!(answered["lifecycle"], "active");
    assert_eq!(answered["auto_resolve"], false);
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
fn starts_reject_escaping_symlinks_without_writing_any_of_the_batch() -> Result<()> {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new("mcp-symlink-escape")?;
    let outside = fixture.dir.0.join("outside");
    fs::create_dir(&outside)?;
    let private_text = "synthetic outside-only evidence\n";
    fs::write(outside.join("evidence.txt"), private_text)?;
    symlink(
        outside.join("evidence.txt"),
        fixture.root.join("absolute.md"),
    )?;
    symlink("../outside/evidence.txt", fixture.root.join("relative.md"))?;
    symlink("../outside", fixture.root.join("linked-directory"))?;
    symlink("relative.md", fixture.root.join("chain.md"))?;
    let mut client = Mcp::copilot(&fixture, "symlink-escape")?;

    for path in [
        "absolute.md",
        "relative.md",
        "linked-directory/evidence.txt",
        "chain.md",
    ] {
        for line in [Some(1), None] {
            let result = client.call(
                "thread_start",
                json!({"comments": [
                    {"path": "a.md", "line": 1, "body": "valid first item"},
                    {"path": path, "line": line, "body": "must not capture outside text"}
                ]}),
            )?;
            assert_eq!(result["isError"], true, "{path}: {result}");
            assert_eq!(result["structuredContent"]["error_code"], "INVALID_BATCH");
            assert_eq!(result["structuredContent"]["issues"][0]["item_index"], 1);
            assert!(!result.to_string().contains(private_text.trim()));
            assert!(fixture.store()?.threads().is_empty());
        }
    }
    Ok(())
}

#[test]
fn starts_accept_relative_symlinks_confined_to_the_checkout() -> Result<()> {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new("mcp-symlink-confined")?;
    fs::create_dir(fixture.root.join("nested"))?;
    fs::write(fixture.root.join("nested/b.md"), "inside\n")?;
    symlink("a.md", fixture.root.join("file-link.md"))?;
    symlink("nested", fixture.root.join("directory-link"))?;
    symlink("../a.md", fixture.root.join("nested/parent-link.md"))?;
    let mut client = Mcp::copilot(&fixture, "symlink-confined")?;

    for (path, snippet) in [
        ("file-link.md", "one"),
        ("directory-link/b.md", "inside"),
        ("nested/parent-link.md", "one"),
    ] {
        let result = client.ok(
            "thread_start",
            json!({"comments": [{"path": path, "line": 1, "body": "inside review"}]}),
        )?;
        let thread = &result["structuredContent"]["threads"][0];
        assert_eq!(thread["path"], path);
        assert_eq!(thread["origin"]["snippet"], snippet);
        assert_eq!(thread["placement"], "anchored");
    }
    assert_eq!(fixture.store()?.threads().len(), 3);
    Ok(())
}

#[test]
fn reads_and_replies_reject_files_replaced_by_escaping_symlinks() -> Result<()> {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new("mcp-symlink-replacement")?;
    let mut client = Mcp::copilot(&fixture, "symlink-replacement")?;
    let started = client.ok(
        "thread_start",
        json!({"comments": [{"path": "a.md", "line": 1, "body": "original review"}]}),
    )?;
    let id = started["structuredContent"]["threads"][0]["id"].clone();
    let before = fs::read(fixture.threads_path()?)?;
    let private_text = "synthetic outside-only evidence\nsecond private line\n";
    fs::write(fixture.dir.0.join("outside.txt"), private_text)?;
    fs::remove_file(fixture.root.join("a.md"))?;
    symlink("../outside.txt", fixture.root.join("a.md"))?;

    for (tool, arguments) in [
        ("threads", json!({"ids": [id]})),
        (
            "thread_reply",
            json!({"replies": [{"thread": id, "line": 2, "body": "move review"}]}),
        ),
    ] {
        let result = client.call(tool, arguments)?;
        assert_eq!(result["isError"], true, "{tool}: {result}");
        assert!(!result.to_string().contains("outside-only evidence"));
        assert!(!result.to_string().contains("second private line"));
        assert_eq!(fs::read(fixture.threads_path()?)?, before);
    }
    Ok(())
}

#[test]
fn replies_use_the_bound_checkout_stored_path_across_renames() -> Result<()> {
    let fixture = Fixture::new("mcp-bound-rename")?;
    let line_thread = fixture.user_thread("line review")?;
    let keyed_thread = fixture.user_thread("keyed review")?;
    let file_thread = Store::open(fixture.threads_path()?)?.annotate(
        Draft::on_file(Author::User, Path::new("a.md"), "file review"),
        "one\ntwo\n",
        now(),
    )?;
    let mut client = Mcp::copilot(&fixture, "rename-chat")?;
    let keyed_request = json!({"replies": [{
        "thread": keyed_thread,
        "line": 2,
        "body": "keyed before rename",
        "idempotency_key": "rename-retry"
    }]});
    client.ok("thread_reply", keyed_request.clone())?;

    fs::rename(fixture.root.join("a.md"), fixture.root.join("renamed.md"))?;
    let before_fresh = fs::read(fixture.threads_path()?)?;
    let fresh = client.call(
        "thread_reply",
        json!({"replies": [{
            "thread": line_thread,
            "line": 1,
            "body": "fresh after rename"
        }]}),
    )?;
    assert_eq!(fresh["isError"], true);
    assert!(
        fresh["content"][0]["text"]
            .as_str()
            .context("missing stored path error")?
            .contains("cannot relocate a.md")
    );
    assert_eq!(
        fs::read(fixture.threads_path()?)?,
        before_fresh,
        "failed fresh line reply mutated the store"
    );

    let file_reply = client.ok(
        "thread_reply",
        json!({"replies": [{"thread": file_thread, "body": "file-wide after rename"}]}),
    )?;
    assert_eq!(
        file_reply["structuredContent"]["results"][0]["thread"]["messages"][1]["body"],
        "file-wide after rename"
    );
    let replay = client.ok("thread_reply", keyed_request)?;
    assert_eq!(replay["structuredContent"]["results"][0]["replayed"], true);
    assert_eq!(
        replay["structuredContent"]["results"][0]["thread"]["messages"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );

    let other = fixture.dir.0.join("viewer-checkout");
    fs::create_dir(&other)?;
    fs::write(other.join("a.md"), "viewer-only line\n")?;
    Record::new("1700000001-1".parse::<Id>()?, fixture.key()?, other).write(&fixture.dirs)?;
    fs::write(fixture.root.join("a.md"), "bound first\nbound second\n")?;
    let recreated = client.ok(
        "thread_reply",
        json!({"replies": [{
            "thread": line_thread,
            "line": 2,
            "body": "placed in recreated bound source"
        }]}),
    )?;
    assert_eq!(
        recreated["structuredContent"]["results"][0]["thread"]["range"],
        json!({"start": 2, "end": 2})
    );
    assert_eq!(
        recreated["structuredContent"]["results"][0]["thread"]["messages"][1]["body"],
        "placed in recreated bound source"
    );
    Ok(())
}

#[test]
fn agent_starts_capture_bound_checkout_working_tree_provenance() -> Result<()> {
    let fixture = Fixture::new("mcp-working-provenance")?;
    fathomable_testing::git::init(&fixture.root)?;
    fathomable_testing::git::commit_and_stage(&fixture.root, &[("a.md", "one\ntwo\n")])?;
    let observed_head = Workspace::discover(&fixture.root)?
        .head_commit()
        .context("observed HEAD")?;
    fs::write(fixture.root.join("a.md"), "one\nchanged\n")?;

    let mut client = Mcp::copilot(&fixture, "dirty-start")?;
    let result = client.ok(
        "thread_start",
        json!({"comments": [{"path": "a.md", "line": 2, "body": "dirty finding"}]}),
    )?;
    assert_eq!(
        result["structuredContent"]["checkout"],
        fixture.root.to_string_lossy().as_ref()
    );
    let shown = &result["structuredContent"]["threads"][0];
    assert_eq!(shown["origin"]["path"], "a.md");
    assert_eq!(shown["origin"]["range"], json!({"start": 2, "end": 2}));
    assert_eq!(shown["origin"]["version"]["kind"], "working_tree");
    assert_eq!(shown["origin"]["version"]["observed_head"], observed_head);
    assert_eq!(shown["origin"]["side"], "unspecified");
    assert_eq!(shown["origin"]["working_tree"]["dirty"], true);
    assert_eq!(shown["origin"]["working_tree"]["added"], false);
    assert_eq!(shown["origin"]["working_tree"]["deleted"], false);
    assert_eq!(shown["origin"]["working_tree"]["content"]["bytes"], 12);
    assert_eq!(shown["origin"]["content"]["bytes"], 7);
    assert!(
        shown["commit"].is_null(),
        "agent starts have no human commit origin"
    );
    Ok(())
}

#[test]
fn archived_threads_leave_normal_reads_but_exact_replay_is_safe() -> Result<()> {
    let fixture = Fixture::new("mcp-archive-replay")?;
    let thread = fixture.user_thread("archive me")?;
    let request = json!({"replies": [{
        "thread": thread,
        "body": "completed before archive",
        "idempotency_key": "archive-replay"
    }]});
    let mut client = Mcp::copilot(&fixture, "archive-chat")?;
    client.ok("thread_reply", request.clone())?;

    let mut store = fixture.store()?;
    store.archive(&thread, now())?;
    drop(store);

    for arguments in [
        json!({}),
        json!({"status": "open"}),
        json!({"status": "all"}),
    ] {
        let result = client.ok("threads", arguments)?;
        assert!(
            result["structuredContent"]["threads"]
                .as_array()
                .context("normal threads")?
                .is_empty()
        );
    }
    let exact = client.ok("threads", json!({"ids": [thread]}))?;
    let exact_thread = &exact["structuredContent"]["threads"][0];
    assert_eq!(exact_thread["archived"], true);
    assert_eq!(
        exact_thread["archive_history"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(exact_thread["messages"].as_array().map(Vec::len), Some(2));

    let fresh = client.call(
        "thread_reply",
        json!({"replies": [{
            "thread": thread,
            "body": "must fail",
            "idempotency_key": "fresh-after-archive"
        }]}),
    )?;
    assert_eq!(fresh["isError"], true);
    assert!(
        fresh["content"][0]["text"]
            .as_str()
            .context("archive error")?
            .contains("archived")
    );
    assert_eq!(
        fixture
            .store()?
            .thread(&thread)
            .context("archived thread")?
            .replies()
            .len(),
        1
    );

    let replay = client.ok("thread_reply", request)?;
    let replayed = &replay["structuredContent"]["results"][0];
    assert_eq!(replayed["replayed"], true);
    assert_eq!(replayed["thread"]["archived"], true);
    assert_eq!(
        replayed["thread"]["messages"].as_array().map(Vec::len),
        Some(2)
    );
    assert_eq!(
        fixture
            .store()?
            .thread(&thread)
            .context("replayed thread")?
            .replies()
            .len(),
        1
    );
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
fn context_projection_preserves_placement_without_read_side_effects() -> Result<()> {
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
    fs::write(fixture.root.join("a.md"), "zero\none\nTWO\nthree\n")?;

    let mut client = Mcp::start(&fixture, "unknown-client", &[])?;
    let path = fixture.threads_path()?;
    let after_startup = fs::read(&path)?;
    let result = client.ok("threads", json!({"ids": [thread]}))?;
    let shown = &result["structuredContent"]["threads"][0];
    assert_eq!(shown["range"], json!({"start": 3, "end": 3}));
    assert_eq!(shown["placement"], "edited");
    assert_eq!(shown["anchor_range"], json!({"start": 2, "end": 2}));
    assert_eq!(shown["location"], "moved");
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
        resolved["structuredContent"]["threads"][0]["messages"][1]["body"],
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
        threads[1]["messages"][1]["resolution_proposed"], true,
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
        started_result["structuredContent"]["threads"][0]["messages"][0]["author"],
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
        json!({"replies": [{"thread": thread, "body": "my proposal", "resolve": true}]}),
    )?;
    assert_eq!(
        replied_result["structuredContent"]["results"][0]["thread"]["messages"][1]["author"],
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
    assert_eq!(
        replied_result["structuredContent"]["results"][0]["resolution"]["outcome"],
        "resolution_proposed"
    );
    assert_eq!(
        replied_result["structuredContent"]["results"][0]["resolution"]["reason"],
        "pending_fathomable_user_review"
    );
    assert!(
        replied_result["structuredContent"]["results"][0]["resolution"]["guidance"]
            .as_str()
            .context("proposal guidance")?
            .contains("do not retry")
    );
    let store = fixture.store()?;
    let started = store.threads().last().context("started")?;
    assert_eq!(started.author().name(), "Copilot");
    assert_eq!(started.author().id(), Some("copilot:chat"));
    let replied = store.thread(&thread).context("replied")?;
    assert_eq!(replied.replies()[0].author().id(), Some("copilot:chat"));
    assert!(replied.replies()[0].proposes_resolution());
    assert_eq!(replied.status(), Status::Open, "agent closed the thread");
    assert_eq!(replied.lifecycle(), Lifecycle::ResolutionProposed);
    Ok(())
}

#[test]
fn reply_resolution_outcomes_cover_authorized_proposed_and_not_requested() -> Result<()> {
    let fixture = Fixture::new("mcp-resolution-outcomes")?;
    fathomable_testing::git::init(&fixture.root)?;
    fathomable_testing::git::commit_and_stage(&fixture.root, &[("a.md", "one\ntwo\n")])?;
    let authorized = fixture.user_thread("finish this")?;
    let ordinary = fixture.user_thread("keep working")?;
    let proposed = fixture.user_thread("review completion")?;
    let head = Workspace::discover(&fixture.root)?
        .head_commit()
        .context("HEAD")?;
    let mut store = fixture.store()?;
    store.set_auto_resolve(&authorized, AutoResolve::Enabled, now())?;
    store.set_auto_resolve(&ordinary, AutoResolve::Enabled, now())?;
    drop(store);

    let mut client = Mcp::copilot(&fixture, "chat")?;
    let resolved = client.ok(
        "thread_reply",
        json!({"replies": [{"thread": authorized, "body": "fixed", "resolve": true}]}),
    )?;
    let resolved = &resolved["structuredContent"]["results"][0];
    assert_eq!(resolved["resolution"]["outcome"], "resolved");
    assert!(resolved["resolution"].get("reason").is_none());
    assert!(resolved["resolution"].get("guidance").is_none());
    assert_eq!(resolved["thread"]["status"], "resolved");
    assert_eq!(resolved["thread"]["lifecycle"], "resolved");
    assert_eq!(resolved["thread"]["auto_resolve"], false);
    assert_eq!(resolved["thread"]["commit"], head);

    let ordinary_result = client.ok(
        "thread_reply",
        json!({"replies": [{"thread": ordinary, "body": "progress", "resolve": false}]}),
    )?;
    let ordinary_result = &ordinary_result["structuredContent"]["results"][0];
    assert_eq!(ordinary_result["resolution"]["outcome"], "not_requested");
    assert!(ordinary_result["resolution"].get("reason").is_none());
    assert_eq!(ordinary_result["thread"]["lifecycle"], "active");
    assert_eq!(ordinary_result["thread"]["auto_resolve"], false);

    let proposed_result = client.ok(
        "thread_reply",
        json!({"replies": [{"thread": proposed, "body": "done", "resolve": true}]}),
    )?;
    let proposed_result = &proposed_result["structuredContent"]["results"][0];
    assert_eq!(
        proposed_result["resolution"]["outcome"],
        "resolution_proposed"
    );
    assert_eq!(
        proposed_result["resolution"]["reason"],
        "pending_fathomable_user_review"
    );
    assert!(
        proposed_result["resolution"]["guidance"]
            .as_str()
            .context("proposal guidance")?
            .to_lowercase()
            .contains("do not ask for confirmation in chat and do not retry")
    );
    assert_eq!(proposed_result["thread"]["status"], "open");
    assert_eq!(
        proposed_result["thread"]["lifecycle"],
        "resolution_proposed"
    );

    let open = client.ok("threads", json!({"status": "open"}))?;
    let open_ids: Vec<&str> = open["structuredContent"]["threads"]
        .as_array()
        .context("open threads")?
        .iter()
        .filter_map(|thread| thread["id"].as_str())
        .collect();
    assert!(open_ids.contains(&ordinary.to_string().as_str()));
    assert!(open_ids.contains(&proposed.to_string().as_str()));
    assert!(!open_ids.contains(&authorized.to_string().as_str()));
    Ok(())
}

#[test]
fn reply_batch_results_follow_request_order() -> Result<()> {
    let fixture = Fixture::new("mcp-reply-order")?;
    let first = fixture.user_thread("first")?;
    let second = fixture.user_thread("second")?;
    let third = fixture.user_thread("third")?;
    let expected = [third.to_string(), first.to_string(), second.to_string()];
    let mut client = Mcp::copilot(&fixture, "chat")?;

    let result = client.ok(
        "thread_reply",
        json!({"replies": [
            {"thread": expected[0], "body": "third answer"},
            {"thread": expected[1], "body": "first answer", "resolve": true},
            {"thread": expected[2], "body": "second answer"}
        ]}),
    )?;

    let results = result["structuredContent"]["results"]
        .as_array()
        .context("reply results")?;
    let actual: Vec<&str> = results
        .iter()
        .filter_map(|result| result["thread"]["id"].as_str())
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(results[1]["resolution"]["outcome"], "resolution_proposed");
    Ok(())
}

#[test]
fn keyed_resolution_replay_returns_current_thread_and_original_outcome() -> Result<()> {
    let fixture = Fixture::new("mcp-resolution-replay")?;
    let thread = fixture.user_thread("is this done?")?;
    let request = json!({"replies": [{
        "thread": thread,
        "body": "done",
        "resolve": true,
        "idempotency_key": "resolution-once"
    }]});
    let mut client = Mcp::copilot(&fixture, "chat")?;
    let first = client.ok("thread_reply", request.clone())?;
    assert_eq!(
        first["structuredContent"]["results"][0]["resolution"]["outcome"],
        "resolution_proposed"
    );

    let mut store = fixture.store()?;
    store.set_auto_resolve(&thread, AutoResolve::Enabled, now())?;
    store.reply_user(&thread, now(), "not yet", UserSubmit::Normal)?;
    let before = fs::read(fixture.threads_path()?)?;
    drop(store);

    let replay = client.ok("thread_reply", request)?;
    let result = &replay["structuredContent"]["results"][0];
    assert_eq!(result["replayed"], true);
    assert_eq!(
        result["resolution"]["outcome"], "resolution_proposed",
        "replay must retain the original durable outcome"
    );
    assert_eq!(result["thread"]["lifecycle"], "active");
    assert_eq!(result["thread"]["auto_resolve"], true);
    assert_eq!(
        result["thread"]["messages"].as_array().map(Vec::len),
        Some(3)
    );
    assert_eq!(
        fs::read(fixture.threads_path()?)?,
        before,
        "replay changed durable state"
    );
    Ok(())
}

#[test]
fn thread_messages_are_uniform_for_edited_and_agent_openings() -> Result<()> {
    let fixture = Fixture::new("mcp-uniform-messages")?;
    let mut store = fixture.store()?;
    let edited = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "original",
        ),
        "one\ntwo\n",
        10,
    )?;
    store.edit(&edited, MessageTarget::Comment, "edited opening", 20)?;
    let agent = store.annotate(
        Draft::new(
            Author::Agent {
                name: "Review bot".to_owned(),
                client: Some("fixture".to_owned()),
                id: Some("fixture:agent".to_owned()),
            },
            Path::new("a.md"),
            LineRange::new(2, 2),
            "agent opening",
        ),
        "one\ntwo\n",
        30,
    )?;
    drop(store);

    let mut client = Mcp::start(&fixture, "unknown-client", &[])?;
    let result = client.ok(
        "threads",
        json!({"ids": [edited.to_string(), agent.to_string()]}),
    )?;
    let threads = result["structuredContent"]["threads"]
        .as_array()
        .context("threads")?;
    let edited_message = &threads[0]["messages"][0];
    assert_eq!(edited_message["body"], "edited opening");
    assert_eq!(edited_message["created"], 10);
    assert_eq!(edited_message["modified"], 20);
    assert_eq!(edited_message["resolution_proposed"], false);
    let keys: std::collections::HashSet<&str> = edited_message
        .as_object()
        .context("message object")?
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        std::collections::HashSet::from([
            "author",
            "body",
            "created",
            "modified",
            "resolution_proposed"
        ])
    );
    assert_eq!(
        threads[1]["messages"][0]["author"],
        json!({
            "kind": "agent",
            "name": "Review bot",
            "client": "fixture",
            "id": "fixture:agent"
        })
    );
    for obsolete in ["author", "comment", "replies", "updated", "edited"] {
        assert!(
            threads[0].get(obsolete).is_none(),
            "obsolete field {obsolete} remains"
        );
    }
    Ok(())
}

#[test]
fn viewer_records_do_not_affect_bound_store_writes() -> Result<()> {
    let fixture = Fixture::new("mcp-record-independent")?;
    let mut client = Mcp::copilot(&fixture, "chat")?;

    let zero = client.ok(
        "thread_start",
        json!({"comments": [{"path": "a.md", "body": "zero records"}]}),
    )?;
    assert_eq!(
        zero["structuredContent"]["threads"][0]["messages"][0]["body"],
        "zero records"
    );
    let reply_target = zero["structuredContent"]["threads"][0]["id"].clone();

    Record::new(
        "1700000000-1".parse::<Id>()?,
        fixture.key()?,
        fixture.root.clone(),
    )
    .write(&fixture.dirs)?;
    let one = client.ok(
        "thread_start",
        json!({"comments": [{"path": "a.md", "body": "one record"}]}),
    )?;
    assert_eq!(
        one["structuredContent"]["threads"][0]["messages"][0]["body"],
        "one record"
    );

    let elsewhere = fixture.dir.0.join("elsewhere");
    fs::create_dir(&elsewhere)?;
    Record::new("1700000000-2".parse::<Id>()?, fixture.key()?, elsewhere).write(&fixture.dirs)?;
    let stale_dir = fixture.dirs.viewers_dir().join("1700000000-4000000");
    fixture.dirs.prepare_state_dir(&stale_dir)?;
    fathomable_core::private_state::write(
        stale_dir.join("session.json"),
        format!(
            r#"{{"id":"1700000000-4000000","pid":4000000,"key":{},"root":{},"socket":"/stale/viewer.sock","started":1}}"#,
            serde_json::to_string(&fixture.key()?)?,
            serde_json::to_string(&fixture.root)?,
        ),
    )?;

    let many = client.ok(
        "thread_start",
        json!({"comments": [{"path": "a.md", "body": "many and stale records"}]}),
    )?;
    assert_eq!(
        many["structuredContent"]["threads"][0]["messages"][0]["body"],
        "many and stale records"
    );
    let reply = client.ok(
        "thread_reply",
        json!({"replies": [{"thread": reply_target, "body": "records still ignored"}]}),
    )?;
    assert_eq!(
        reply["structuredContent"]["results"][0]["thread"]["messages"][1]["body"],
        "records still ignored"
    );
    assert_eq!(fixture.store()?.threads().len(), 3);
    assert_eq!(Record::list(&fixture.dirs).len(), 3);
    Ok(())
}

#[test]
fn batch_validation_writes_nothing_when_any_item_is_invalid() -> Result<()> {
    let fixture = Fixture::new("mcp-batch")?;
    let thread = fixture.user_thread("answer me")?;
    let oversized_thread = fixture.user_thread("answer briefly")?;
    let mut client = Mcp::copilot(&fixture, "chat")?;
    let start = client.call(
        "thread_start",
        json!({"comments": [
            {"path": "a.md", "body": "valid"},
            {"path": "missing.md", "body": "invalid"},
            {"path": "a.md", "body": "x".repeat(1025)}
        ]}),
    )?;
    assert_eq!(start["isError"], true);
    assert_eq!(start["structuredContent"]["error_code"], "INVALID_BATCH");
    assert_eq!(start["structuredContent"]["issues"][0]["item_index"], 1);
    assert_eq!(start["structuredContent"]["issues"][1]["item_index"], 2);
    assert!(
        start["structuredContent"]["issues"][1]["message"]
            .as_str()
            .context("oversized start error")?
            .contains("maximum is 1024")
    );
    assert!(
        start["content"][0]["text"]
            .as_str()
            .context("start batch error")?
            .contains("comments[1]")
    );
    assert_eq!(
        serde_json::from_str::<Value>(
            start["content"][0]["text"]
                .as_str()
                .context("start batch JSON fallback")?
        )?,
        start["structuredContent"]
    );
    assert_eq!(fixture.store()?.threads().len(), 2);

    let reply = client.call(
        "thread_reply",
        json!({"replies": [
            {"thread": thread, "body": "valid"},
            {"thread": "not-a-thread", "body": "invalid"},
            {"thread": oversized_thread, "body": "x".repeat(1025)}
        ]}),
    )?;
    assert_eq!(reply["isError"], true);
    assert_eq!(reply["structuredContent"]["error_code"], "INVALID_BATCH");
    assert_eq!(reply["structuredContent"]["issues"][0]["item_index"], 1);
    assert_eq!(reply["structuredContent"]["issues"][1]["item_index"], 2);
    let oversized_reply = reply["structuredContent"]["issues"][1]["message"]
        .as_str()
        .context("oversized reply error")?;
    assert!(
        oversized_reply.contains("maximum is 1024"),
        "{oversized_reply}"
    );
    assert!(
        reply["content"][0]["text"]
            .as_str()
            .context("reply batch error")?
            .contains("replies[1]")
    );
    assert_eq!(
        serde_json::from_str::<Value>(
            reply["content"][0]["text"]
                .as_str()
                .context("reply batch JSON fallback")?
        )?,
        reply["structuredContent"]
    );
    assert!(
        fixture
            .store()?
            .thread(&thread)
            .context("thread")?
            .replies()
            .is_empty()
    );
    assert!(
        fixture
            .store()?
            .thread(&oversized_thread)
            .context("oversized thread")?
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
            json!({"replies": [{"thread": thread, "body": "x", "propose_resolve": true}]}),
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
        1,
        "resolved board membership is not gated by current HEAD ancestry"
    );
    let exact = client.ok("threads", json!({"ids": [thread]}))?;
    assert_eq!(
        exact["structuredContent"]["threads"][0]["messages"][0]["body"],
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

    let shown = &result["structuredContent"]["results"][0]["thread"];
    assert_eq!(shown["placement"], "anchored");
    assert_eq!(shown["range"], json!({"start": 1, "end": 2}));
    assert_eq!(shown["anchor_range"], json!({"start": 1, "end": 2}));
    assert!(shown["reanchored_at"].is_null());
    Ok(())
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one linked-worktree fixture covers reads, batch refusal, reply, replay, and resolution"
)]
fn review_reads_stay_bound_to_the_startup_checkout() -> Result<()> {
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
    let mut store = Store::open(fixture.dirs.threads_file(workspace.key()))?;
    let id = store.annotate(
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
    let authorized = store.annotate(
        Draft::new(
            Author::User,
            Path::new("feature.md"),
            LineRange::new(1, 1),
            "finish this branch",
        )
        .at_commit(workspace.head_commit()),
        "branch-only finding\n",
        now(),
    )?;
    store.set_auto_resolve(&authorized, AutoResolve::Enabled, now())?;
    drop(store);
    let result = client.ok("threads", json!({}))?;
    let shown = result["structuredContent"]["threads"]
        .as_array()
        .context("threads")?
        .iter()
        .find(|thread| thread["id"] == id.to_string())
        .context("shared worktree discussion missing")?;
    assert_eq!(shown["placement"], "detached");
    assert!(shown["worktree"].is_null());
    assert_eq!(shown["origin"]["path"], "feature.md");

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
    let reply = json!({"replies": [
        {
            "thread": id,
            "body": "updated on the feature branch",
            "idempotency_key": "feature-reply"
        },
        {
            "thread": authorized,
            "body": "relocate before resolving",
            "resolve": true,
            "line": 2
        }
    ]});
    let rejected = client.call("thread_reply", reply)?;
    assert_eq!(rejected["isError"], true);
    assert_eq!(rejected["structuredContent"]["error_code"], "INVALID_BATCH");
    assert_eq!(rejected["structuredContent"]["issues"][0]["item_index"], 1);
    let error = rejected["structuredContent"]["issues"][0]["message"]
        .as_str()
        .context("bound checkout reply error")?;
    assert!(
        error.contains("cannot relocate")
            && error.contains("omit `line`")
            && error.contains("run MCP bound to a worktree"),
        "{error}"
    );
    let store = Store::open(fixture.dirs.threads_file(workspace.key()))?;
    assert!(
        store
            .thread(&id)
            .context("historical branch thread")?
            .replies()
            .is_empty()
    );
    let authorized_thread = store
        .thread(&authorized)
        .context("authorized branch thread")?;
    assert!(authorized_thread.replies().is_empty());
    assert_eq!(authorized_thread.auto_resolve(), AutoResolve::Enabled);
    drop(store);

    let reply = json!({"replies": [{
        "thread": id,
        "body": "updated on the feature branch",
        "idempotency_key": "feature-reply"
    }]});
    let replied = client.ok("thread_reply", reply.clone())?;
    let replied_result = &replied["structuredContent"]["results"][0];
    assert_eq!(replied_result["replayed"], false);
    assert_eq!(replied_result["resolution"]["outcome"], "not_requested");
    let replied_thread = &replied_result["thread"];
    for field in [
        "path",
        "range",
        "placement",
        "location",
        "anchor_range",
        "origin",
        "placement_evidence",
        "reanchored_at",
    ] {
        assert_eq!(replied_thread[field], shown[field], "reply changed {field}");
    }
    assert_eq!(
        replied["content"][0]["text"],
        replied["structuredContent"].to_string()
    );

    let replayed = client.ok("thread_reply", reply)?;
    assert_eq!(
        replayed["structuredContent"]["results"][0]["replayed"],
        true
    );
    assert_eq!(
        replayed["structuredContent"]["results"][0]["thread"]["messages"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );

    let bound_head = Workspace::discover(&fixture.root)?
        .head_commit()
        .context("bound HEAD")?;
    let resolved = client.ok(
        "thread_reply",
        json!({"replies": [{
            "thread": authorized,
            "body": "finished on the feature branch",
            "resolve": true
        }]}),
    )?;
    let resolved = &resolved["structuredContent"]["results"][0];
    assert_eq!(resolved["resolution"]["outcome"], "resolved");
    assert_eq!(resolved["thread"]["status"], "resolved");
    assert_eq!(resolved["thread"]["placement"], "detached");
    assert_eq!(resolved["thread"]["commit"], bound_head);

    let store = Store::open(fixture.dirs.threads_file(workspace.key()))?;
    let thread = store.thread(&id).context("replied branch thread")?;
    assert_eq!(thread.author(), &Author::User);
    assert_eq!(thread.replies().len(), 1);
    assert_eq!(thread.range(), Some(LineRange::new(1, 1)));
    assert_eq!(
        store
            .thread(&authorized)
            .context("resolved branch thread")?
            .replies()
            .len(),
        1
    );
    assert!(
        !fixture.root.join("feature.md").exists(),
        "replies created the sibling-only file in the bound checkout"
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
