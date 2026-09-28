use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use fathomable_core::annotations::{
    Author, Draft, LandingOutcome, LineRange, MessageTarget, Reply, Store, Thread, ThreadId,
};
use fathomable_core::clock::now;
use fathomable_core::workspace::{CommitId, Workspace};
use serde_json::{Value, json};

use super::{Fixture, Mcp, schema_contains_values, schema_has_property};

fn changes(result: &Value) -> &Value {
    &result["structuredContent"]["changes"]
}

fn hints(client: &mut Mcp) -> Result<Value> {
    client.ok("threads", json!({"limit": 0}))
}

#[test]
fn change_hints_distinguish_startup_new_replies_and_edits_without_clock_ticks() -> Result<()> {
    let fixture = Fixture::new("mcp-change-hints")?;
    let existing = fixture.user_thread("existing")?;
    let resolved = fixture.user_thread("already resolved")?;
    let archived = fixture.user_thread("already archived")?;
    let mut store = fixture.store()?;
    store.resolve(&resolved, None, now())?;
    store.archive(&archived, now())?;
    let mut client = Mcp::copilot(&fixture, "reader")?;

    assert_eq!(
        changes(&hints(&mut client)?),
        &json!([[existing, "existing"]])
    );
    assert!(
        hints(&mut client)?["structuredContent"]
            .get("changes")
            .is_none()
    );
    let when = now();
    let added = store.annotate(
        Draft::new(Author::User, Path::new("a.md"), LineRange::new(1, 1), "new"),
        "one\ntwo\n",
        when,
    )?;
    assert_eq!(changes(&hints(&mut client)?), &json!([[added, "new"]]));
    store.reply(&added, Reply::new(Author::User, when, "reply"))?;
    assert_eq!(changes(&hints(&mut client)?), &json!([[added, "add"]]));
    store.edit(&added, MessageTarget::Reply(0), "edited reply", when)?;
    assert_eq!(changes(&hints(&mut client)?), &json!([[added, "edit"]]));
    store.edit(&added, MessageTarget::Reply(0), "edited again", when)?;
    assert_eq!(changes(&hints(&mut client)?), &json!([[added, "edit"]]));
    assert!(changes(&hints(&mut client)?).is_null());
    Ok(())
}

#[test]
fn change_hints_coalesce_and_include_lifecycle_changes_outside_read_filters() -> Result<()> {
    let fixture = Fixture::new("mcp-change-lifecycle")?;
    let id = fixture.user_thread("original")?;
    let mut client = Mcp::copilot(&fixture, "reader")?;
    hints(&mut client)?;
    let when = now();
    let mut store = fixture.store()?;
    store.reply(&id, Reply::new(Author::User, when, "first reply"))?;
    store.reply(&id, Reply::new(Author::User, when, "second reply"))?;
    store.resolve(&id, None, when)?;
    assert_eq!(changes(&hints(&mut client)?), &json!([[id, "resolve"]]));
    store.reopen(&id, when)?;
    assert_eq!(changes(&hints(&mut client)?), &json!([[id, "reopen"]]));
    store.archive(&id, when)?;
    assert_eq!(changes(&hints(&mut client)?), &json!([[id, "archive"]]));
    store.restore(&id, when)?;
    assert_eq!(changes(&hints(&mut client)?), &json!([[id, "restore"]]));
    store.delete(&id, when)?;
    assert_eq!(changes(&hints(&mut client)?), &json!([[id, "delete"]]));
    Ok(())
}

#[test]
fn change_hints_ignore_landing_only_but_keep_replies_and_edits() -> Result<()> {
    let fixture = Fixture::new("mcp-change-landing")?;
    fathomable_testing::git::init(&fixture.root)?;
    fathomable_testing::git::commit_and_stage(&fixture.root, &[("a.md", "one\ntwo\n")])?;
    let commit = CommitId::parse(
        &Workspace::discover(&fixture.root)?
            .head_commit()
            .context("HEAD")?,
    )?;
    let mut client = Mcp::copilot(&fixture, "reader")?;
    let started = client.ok(
        "thread_start",
        json!({"comments": [
            {"path": "a.md", "body": "first"},
            {"path": "a.md", "body": "second"},
            {"path": "a.md", "body": "untouched"}
        ]}),
    )?;
    let ids = started["structuredContent"]["threads"]
        .as_array()
        .context("started threads")?
        .iter()
        .map(|thread| serde_json::from_value::<ThreadId>(thread["id"].clone()))
        .collect::<Result<Vec<_>, _>>()?;

    let mut store = fixture.store()?;
    let candidates = ids
        .iter()
        .map(|id| {
            store
                .thread(id)
                .and_then(Thread::landing_candidate)
                .context("landing candidate")
        })
        .collect::<Result<Vec<_>>>()?;
    assert_eq!(
        store.land(&candidates[0], &commit)?,
        LandingOutcome::Applied
    );
    assert!(
        hints(&mut client)?["structuredContent"]
            .get("changes")
            .is_none(),
        "landing alone must not produce an edit hint"
    );
    assert_eq!(
        fixture
            .store()?
            .thread(&ids[0])
            .and_then(Thread::landed_commit),
        Some(commit.as_str()),
        "suppressing hints must preserve durable landing evidence"
    );

    for candidate in &candidates[1..] {
        assert_eq!(store.land(candidate, &commit)?, LandingOutcome::Applied);
    }
    let when = now();
    store.reply(&ids[0], Reply::new(Author::User, when, "user reply"))?;
    store.reply(
        &ids[1],
        Reply::new(Author::agent("Copilot"), when, "agent reply"),
    )?;
    assert_eq!(
        changes(&hints(&mut client)?),
        &json!([[ids[0], "add"], [ids[1], "add"]]),
        "landing must not announce the untouched third thread"
    );
    store.edit(&ids[0], MessageTarget::Reply(0), "user edit", when)?;
    assert_eq!(changes(&hints(&mut client)?), &json!([[ids[0], "edit"]]));
    assert!(
        hints(&mut client)?["structuredContent"]
            .get("changes")
            .is_none()
    );
    Ok(())
}

#[test]
fn change_hints_are_per_vscode_conversation_and_copilot_process() -> Result<()> {
    let fixture = Fixture::new("mcp-change-callers")?;
    let mut vscode = Mcp::start(&fixture, "Visual Studio Code", &[])?;
    let mut copilot = Mcp::copilot(&fixture, "copilot-reader")?;
    for chat in ["a", "b"] {
        vscode.meta = json!({"vscode.conversationId": chat});
        hints(&mut vscode)?;
    }
    hints(&mut copilot)?;
    let id = fixture.user_thread("both must notice")?;
    for chat in ["a", "b"] {
        vscode.meta = json!({"vscode.conversationId": chat});
        assert_eq!(changes(&hints(&mut vscode)?), &json!([[id, "new"]]));
        assert!(changes(&hints(&mut vscode)?).is_null());
    }
    assert_eq!(changes(&hints(&mut copilot)?), &json!([[id, "new"]]));
    drop(copilot);
    let mut restarted = Mcp::copilot(&fixture, "copilot-reader")?;
    assert_eq!(changes(&hints(&mut restarted)?), &json!([[id, "existing"]]));
    Ok(())
}

#[test]
fn change_hints_accompany_writes_replays_and_application_errors() -> Result<()> {
    let fixture = Fixture::new("mcp-change-every-tool")?;
    let mut client = Mcp::copilot(&fixture, "writer")?;
    hints(&mut client)?;
    let start = json!({"comments": [{
        "path": "a.md", "body": "finding", "idempotency_key": "start"
    }]});
    let result = client.ok("thread_start", start.clone())?;
    let id = result["structuredContent"]["threads"][0]["id"].clone();
    assert_eq!(changes(&result), &json!([[id, "new"]]));
    assert!(changes(&client.ok("thread_start", start)?).is_null());
    let reply = json!({"replies": [{
        "thread": id, "body": "response", "idempotency_key": "reply"
    }]});
    assert_eq!(
        changes(&client.ok("thread_reply", reply.clone())?),
        &json!([[id, "add"]])
    );
    assert!(changes(&client.ok("thread_reply", reply)?).is_null());

    let user = fixture.user_thread("notice despite invalid batch")?;
    let failed = client.call(
        "thread_start",
        json!({"comments": [
            {"path": "a.md", "body": "x".repeat(1025)}
        ]}),
    )?;
    assert_eq!(failed["isError"], true);
    assert_eq!(changes(&failed), &json!([[user, "new"]]));
    assert!(changes(&hints(&mut client)?).is_null());

    let next = fixture.user_thread("notice despite plain error")?;
    let failed = client.call("threads", json!({"ids": ["missing"]}))?;
    assert_eq!(failed["isError"], true);
    let visible = failed["content"].to_string();
    assert!(visible.contains(next.as_str()), "{failed}");
    assert!(visible.contains("changes"), "{failed}");
    assert!(changes(&hints(&mut client)?).is_null());
    Ok(())
}

#[test]
fn change_hints_fallback_does_not_borrow_or_consume_another_identity() -> Result<()> {
    let fixture = Fixture::new("mcp-change-unavailable")?;
    let mut client = Mcp::start(&fixture, "Visual Studio Code", &[])?;
    client.meta = json!({"vscode.conversationId": "identified"});
    hints(&mut client)?;
    let id = fixture.user_thread("still unseen")?;
    for meta in [
        json!({}),
        json!({"vscode.conversationId": ""}),
        json!({"vscode.conversationId": 7}),
    ] {
        client.meta = meta;
        let result = hints(&mut client)?;
        let fallback = changes(&result).as_str().context("fallback hint")?;
        assert!(fallback.contains("threads") && fallback.contains("since"));
        assert!(
            result["structuredContent"]["threads"]
                .as_array()
                .context("threads")?
                .is_empty()
        );
    }
    client.meta = json!({"vscode.conversationId": "identified"});
    assert_eq!(changes(&hints(&mut client)?), &json!([[id, "new"]]));
    let mut unknown = Mcp::start(&fixture, "unknown", &[])?;
    assert!(
        changes(&hints(&mut unknown)?)
            .as_str()
            .context("unknown caller")?
            .contains("since")
    );
    let mut missing_cli_id = Mcp::start(&fixture, "copilot-cli", &[])?;
    assert!(
        changes(&hints(&mut missing_cli_id)?)
            .as_str()
            .context("missing CLI ID")?
            .contains("since")
    );
    Ok(())
}

#[test]
fn change_hints_are_scoped_to_the_selected_repository_not_the_last_call() -> Result<()> {
    let fixture = Fixture::new("mcp-change-roots")?;
    let other = fixture.dir.0.join("other");
    fs::create_dir(&other)?;
    fs::write(other.join("a.md"), "one\ntwo\n")?;
    let mut client = Mcp::mutable(&fixture, "reader")?;
    hints(&mut client)?;
    client.ok("threads", json!({"workspace": other, "limit": 0}))?;
    let id = fixture.user_thread("default repository")?;
    assert!(changes(&client.ok("threads", json!({"workspace": other}))?).is_null());
    assert_eq!(changes(&hints(&mut client)?), &json!([[id, "new"]]));
    let key = Workspace::discover(&other)?.key().to_path_buf();
    let other_id = Store::open_workspace(&fixture.dirs, &key)?.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "other",
        ),
        "one\ntwo\n",
        now(),
    )?;
    let failed = client.call("threads", json!({"workspace": other.join("absent")}))?;
    assert_eq!(failed["isError"], true);
    assert!(!failed.to_string().contains(other_id.as_str()));
    assert!(changes(&hints(&mut client)?).is_null());
    assert_eq!(
        changes(&client.ok("threads", json!({"workspace": other, "limit": 0}))?),
        &json!([[other_id, "new"]])
    );
    Ok(())
}

#[test]
fn change_hints_preserve_checkpoint_when_store_is_missing_or_corrupt() -> Result<()> {
    let fixture = Fixture::new("mcp-change-read-failure")?;
    fixture.user_thread("original")?;
    let mut client = Mcp::copilot(&fixture, "reader")?;
    hints(&mut client)?;
    let id = fixture.user_thread("pending")?;
    let path = fixture.threads_path()?;
    let saved = fs::read(&path)?;
    let backup = path.with_extension("saved");
    fs::rename(&path, &backup)?;
    let missing = hints(&mut client)?;
    assert!(
        changes(&missing)
            .as_str()
            .context("missing store hint")?
            .contains("unavailable")
    );
    fs::rename(&backup, &path)?;
    fs::write(&path, b"not JSON\n")?;
    let corrupt = client.call("threads", json!({}))?;
    assert_eq!(corrupt["isError"], true);
    assert!(corrupt["content"].to_string().contains("unavailable"));
    fs::write(&path, &saved)?;
    assert_eq!(changes(&hints(&mut client)?), &json!([[id, "new"]]));
    Ok(())
}

#[test]
fn change_hints_are_advertised_but_not_consumed_by_tool_discovery() -> Result<()> {
    let fixture = Fixture::new("mcp-change-schemas")?;
    let mut client = Mcp::copilot(&fixture, "reader")?;
    let id = fixture.user_thread("existing on first business call")?;
    let tools = client.request("tools/list", json!({}))?;
    assert!(!tools.to_string().contains(id.as_str()));
    for tool in tools["tools"].as_array().context("tools")? {
        let schema = &tool["outputSchema"];
        assert!(schema_has_property(schema, "changes"), "{tool}");
        assert!(
            schema_contains_values(
                schema,
                &[
                    "existing", "new", "add", "edit", "resolve", "reopen", "archive", "restore",
                    "delete"
                ]
            ),
            "{tool}"
        );
    }
    assert_eq!(changes(&hints(&mut client)?), &json!([[id, "existing"]]));
    Ok(())
}

#[test]
fn change_hints_linked_worktrees_share_a_checkpoint() -> Result<()> {
    let fixture = Fixture::new("mcp-change-worktrees")?;
    fathomable_testing::git::init(&fixture.root)?;
    fathomable_testing::git::commit_and_stage(&fixture.root, &[("a.md", "one\ntwo\n")])?;
    let linked = fixture.dir.0.join("linked");
    fathomable_testing::git::worktree_add(&fixture.root, &linked, "linked")?;
    let mut client = Mcp::mutable(&fixture, "reader")?;
    hints(&mut client)?;
    let id = fixture.user_thread("shared history")?;
    assert_eq!(
        changes(&client.ok("threads", json!({"workspace": linked, "limit": 0}))?),
        &json!([[id, "new"]])
    );
    assert!(changes(&hints(&mut client)?).is_null());
    Ok(())
}

#[test]
fn change_hints_keep_new_threads_coalesced_and_detect_repeated_lifecycle_events() -> Result<()> {
    let fixture = Fixture::new("mcp-change-coalescing")?;
    let mut client = Mcp::copilot(&fixture, "reader")?;
    hints(&mut client)?;
    let when = now();
    let id = fixture.user_thread("new")?;
    let mut store = fixture.store()?;
    store.reply(&id, Reply::new(Author::User, when, "new reply"))?;
    store.edit(&id, MessageTarget::Reply(0), "corrected", when)?;
    assert_eq!(changes(&hints(&mut client)?), &json!([[id, "new"]]));
    store.resolve(&id, None, when)?;
    store.reopen(&id, when)?;
    assert_eq!(changes(&hints(&mut client)?), &json!([[id, "reopen"]]));
    store.archive(&id, when)?;
    store.restore(&id, when)?;
    assert_eq!(changes(&hints(&mut client)?), &json!([[id, "restore"]]));
    Ok(())
}

#[test]
fn change_hints_fail_explicitly_on_regressed_history_and_recover_without_loss() -> Result<()> {
    let fixture = Fixture::new("mcp-change-regression")?;
    let id = fixture.user_thread("original")?;
    let mut client = Mcp::copilot(&fixture, "reader")?;
    hints(&mut client)?;
    let path = fixture.threads_path()?;
    let earlier = fs::read(&path)?;
    fixture
        .store()?
        .reply(&id, Reply::new(Author::User, now(), "seen"))?;
    hints(&mut client)?;
    fixture
        .store()?
        .reply(&id, Reply::new(Author::User, now(), "pending"))?;
    let saved = fs::read(&path)?;
    fs::write(&path, &earlier)?;
    let regressed = hints(&mut client)?;
    assert!(
        changes(&regressed)
            .as_str()
            .context("regression hint")?
            .contains("unavailable")
    );
    fs::write(&path, &saved)?;
    assert_eq!(changes(&hints(&mut client)?), &json!([[id, "add"]]));
    Ok(())
}
