use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use fathomable_testing::TempDir;

use crate::reach::Reach;

use super::{
    Anchor, Author, Draft, Event, FORMAT_VERSION, LineHashes, LineRange, MessageTarget, Placement,
    Reply, Status, Store, StoreError, Thread, ThreadId, line_hash,
};

const TEXT: &str = "# Title\n\nalpha\nbeta\ngamma\n\ndelta\n";

/// A store path two directories deep inside a fresh temp dir, so a
/// test sees the store create its parents.
struct TempFile(
    PathBuf,
    #[expect(dead_code, reason = "held for its Drop")] TempDir,
);

impl TempFile {
    fn new(name: &str) -> Result<Self, StoreError> {
        let dir = TempDir::new(&format!("annotations-{name}"))
            .map_err(|e| StoreError::io(Path::new(name), e))?;
        Ok(Self(dir.0.join("nested").join("threads.jsonl"), dir))
    }
}

/// An event the file refuses is not kept in memory either: the store
/// shows what a reload would read, not a reply that never landed.
#[test]
fn an_event_the_file_refuses_leaves_the_store_as_it_was() -> Result<(), StoreError> {
    let file = TempFile::new("refused")?;
    let io = |e| StoreError::io(&file.0, e);
    let mut store = Store::open(&file.0)?;
    let draft = |comment: &str| {
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(3, 3),
            comment,
        )
    };
    let first = store.annotate(draft("kept"), TEXT, 1)?;
    // A directory in the file's place refuses the append, whoever runs
    // the test.
    let aside = file.0.with_extension("aside");
    fs::rename(&file.0, &aside).map_err(io)?;
    fs::create_dir(&file.0).map_err(io)?;
    let refused = store.annotate(draft("lost"), TEXT, 2);
    assert!(
        refused.as_ref().is_err_and(StoreError::is_io),
        "{refused:?}"
    );
    assert_eq!(store.threads().len(), 1);
    assert_eq!(store.thread(&first).map(Thread::comment), Some("kept"));
    let reply = store.reply(&first, Reply::new(Author::agent("bot"), 3, "lost too"));
    assert!(reply.is_err());
    assert_eq!(store.thread(&first).map(|t| t.replies().len()), Some(0));
    // With the file back, the next event lands and a reload agrees.
    fs::remove_dir(&file.0).map_err(io)?;
    fs::rename(&aside, &file.0).map_err(io)?;
    store.reply(&first, Reply::new(Author::agent("bot"), 4, "landed"))?;
    let reloaded = Store::open(&file.0)?;
    assert_eq!(reloaded.threads().len(), 1);
    assert_eq!(reloaded.thread(&first).map(|t| t.replies().len()), Some(1));
    Ok(())
}

/// A file of another format version is refused with the line, both
/// versions, and the path to delete; a missing file and one of the
/// current version open (ADR 0062).
#[test]
fn a_store_of_another_format_version_is_refused() -> Result<(), StoreError> {
    let file = TempFile::new("version")?;
    assert!(Store::open(&file.0)?.threads().is_empty());
    let mut current_store = Store::open(&file.0)?;
    current_store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "current",
        ),
        TEXT,
        1,
    )?;
    let current = fs::read_to_string(&file.0).map_err(|e| StoreError::io(&file.0, e))?;
    let stale = current.replace(&format!(r#""v":{FORMAT_VERSION}"#), r#""v":1"#);
    fs::write(&file.0, stale).map_err(|e| StoreError::io(&file.0, e))?;
    let error = Store::open(&file.0).err().map(|e| e.to_string());
    assert_eq!(
        error,
        Some(format!(
            "threads.jsonl line 1: format version 1, this build writes {FORMAT_VERSION}; delete {} to start over",
            file.0.display()
        ))
    );
    fs::write(&file.0, current).map_err(|e| StoreError::io(&file.0, e))?;
    assert_eq!(Store::open(&file.0)?.threads().len(), 1);
    Ok(())
}

/// Current line records carry their complete placement shape; omitting any
/// one of its range, anchor, or context is corruption rather than a fallback.
#[test]
fn a_line_annotation_requires_its_complete_shape() -> Result<(), Box<dyn std::error::Error>> {
    let file = TempFile::new("line-shape")?;
    Store::open(&file.0)?.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "current",
        ),
        TEXT,
        1,
    )?;
    let current = fs::read_to_string(&file.0)?;

    for missing in ["range", "anchor", "context"] {
        let mut event: serde_json::Value = serde_json::from_str(current.trim_end())?;
        event
            .as_object_mut()
            .ok_or("annotation event is not an object")?
            .remove(missing);
        fs::write(&file.0, format!("{event}\n"))?;
        let error = Store::open(&file.0)
            .err()
            .ok_or("incomplete line annotation was accepted")?;
        assert!(
            error
                .to_string()
                .contains("line annotations must carry a range, anchor, and context"),
            "{missing}: {error}"
        );
    }
    Ok(())
}

/// One hashing of a text serves every anchor located in it, moved
/// lines included, exactly as locating from the text does.
#[test]
fn hashes_of_one_text_locate_many_anchors() -> Result<(), String> {
    let alpha = Anchor::capture(TEXT, LineRange::new(3, 4)).ok_or("in range")?;
    let delta = Anchor::capture(TEXT, LineRange::new(7, 7)).ok_or("in range")?;
    let shifted = format!("intro\n{TEXT}");
    let hashes = LineHashes::of(&shifted);
    assert_eq!(hashes.len(), 8);
    assert!(!hashes.is_empty());
    assert_eq!(
        alpha.locate_in(&hashes, LineRange::new(3, 4)),
        Some(LineRange::new(4, 5))
    );
    assert_eq!(
        delta.locate_in(&hashes, LineRange::new(7, 7)),
        Some(LineRange::new(8, 8))
    );
    assert_eq!(
        delta.locate_in(&hashes, LineRange::new(7, 7)),
        delta.locate(&shifted, LineRange::new(7, 7))
    );
    assert!(LineHashes::of("").is_empty());
    Ok(())
}

#[test]
fn line_hash_ignores_trailing_whitespace_only() {
    assert_eq!(line_hash("alpha"), line_hash("alpha  \t"));
    assert_ne!(line_hash("alpha"), line_hash(" alpha"));
    assert_eq!(line_hash("x").len(), 16);
}

#[test]
fn line_range_orders_and_displays() {
    let range = LineRange::new(5, 3);
    assert_eq!((range.start(), range.end(), range.len()), (3, 5, 3));
    assert!(range.contains(4) && !range.contains(6));
    assert_eq!(range.to_string(), "3-5");
    assert_eq!(LineRange::new(0, 0).to_string(), "1");
}

#[test]
fn thread_waits_after_agent_reply_until_user_answers() -> Result<(), StoreError> {
    let file = TempFile::new("waiting")?;
    let mut store = Store::open(&file.0)?;
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(3, 3),
            "why?",
        ),
        TEXT,
        10,
    )?;
    let waiting = |store: &Store| store.thread(&id).is_some_and(Thread::awaits_user);
    assert!(!waiting(&store), "a fresh comment is the user's own");
    store.reply(&id, Reply::new(Author::agent("claude"), 11, "because"))?;
    assert!(waiting(&store));
    store.reply(&id, Reply::new(Author::User, 12, "ok"))?;
    assert!(!waiting(&store));
    store.reply(&id, Reply::new(Author::agent("claude"), 13, "done"))?;
    store.resolve(&id, None, 14)?;
    assert!(!waiting(&store), "a resolved thread never waits");
    Ok(())
}

#[test]
fn user_messages_can_be_edited_and_other_messages_cannot() -> Result<(), StoreError> {
    let file = TempFile::new("edit")?;
    let mut store = Store::open(&file.0)?;
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(3, 3),
            "why?",
        ),
        TEXT,
        10,
    )?;
    store.reply(&id, Reply::new(Author::agent("claude"), 11, "because"))?;
    store.reply(&id, Reply::new(Author::User, 12, "okay"))?;

    store.edit(&id, MessageTarget::Comment, "why exactly?", 13)?;
    store.edit(&id, MessageTarget::Reply(1), "understood", 14)?;
    let before_invalid =
        fs::read_to_string(&file.0).map_err(|error| StoreError::io(&file.0, error))?;
    assert!(
        store
            .edit(&id, MessageTarget::Reply(0), "not mine", 15)
            .is_err()
    );
    assert!(
        store
            .edit(&id, MessageTarget::Reply(2), "missing", 15)
            .is_err()
    );
    assert_eq!(
        fs::read_to_string(&file.0).map_err(|error| StoreError::io(&file.0, error))?,
        before_invalid,
        "invalid edits are not appended"
    );

    let reloaded = Store::open(&file.0)?;
    let thread = reloaded.thread(&id).ok_or_else(|| StoreError {
        kind: super::ErrorKind::UnknownThread(id.clone()),
    })?;
    assert_eq!(thread.comment(), "why exactly?");
    assert_eq!(thread.replies()[0].body(), "because");
    assert_eq!(thread.replies()[1].body(), "understood");
    assert_eq!(thread.updated(), 14);
    Ok(())
}

/// A comment on the file as a whole (ADR 0063): no range, anchor,
/// or snippet in the record, placed as `File` in any text, refused
/// by the moves that need lines, and read back the same.
#[test]
fn a_comment_on_the_file_has_no_lines() -> Result<(), StoreError> {
    let file = TempFile::new("on-file")?;
    let mut store = Store::open(&file.0)?;
    let id = store.annotate(
        Draft::on_file(Author::User, Path::new("a.md"), "split this"),
        TEXT,
        10,
    )?;
    let thread = store.thread(&id).ok_or_else(|| StoreError {
        kind: super::ErrorKind::UnknownThread(id.clone()),
    })?;
    assert!(thread.is_on_file());
    assert_eq!(thread.range(), None);
    assert!(thread.anchor().is_none());
    assert_eq!(thread.snippet(), "");
    assert!(thread.context().is_none());
    assert_eq!(thread.place(), "a.md");
    assert_eq!(thread.locate(TEXT), Placement::File);
    assert_eq!(thread.locate(""), Placement::File);
    assert_eq!(Placement::File.range(), None);
    let record = fs::read_to_string(&file.0).map_err(|e| StoreError::io(&file.0, e))?;
    assert!(
        !record.contains("\"range\"") && !record.contains("\"anchor\""),
        "{record}"
    );
    // The moves that need lines refuse it.
    let moved = store.relocate(&id, LineRange::new(2, 2), TEXT, 11);
    assert!(
        moved
            .as_ref()
            .is_err_and(|e| e.to_string().contains("on the file as a whole")),
        "{moved:?}"
    );
    // It replies and resolves as any thread does, and reads back.
    store.reply(&id, Reply::new(Author::agent("claude"), 12, "done"))?;
    store.resolve(&id, None, 13)?;
    let reloaded = Store::open(&file.0)?;
    let thread = reloaded.thread(&id).ok_or_else(|| StoreError {
        kind: super::ErrorKind::UnknownThread(id.clone()),
    })?;
    assert_eq!(thread.range(), None);
    assert_eq!(thread.status(), Status::Resolved);
    assert_eq!(thread.replies().len(), 1);
    Ok(())
}

#[test]
fn a_deleted_thread_is_gone_and_later_events_on_it_are_ignored() -> Result<(), StoreError> {
    let file = TempFile::new("delete")?;
    let mut store = Store::open(&file.0)?;
    let keep = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "keep",
        ),
        TEXT,
        10,
    )?;
    let gone = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(3, 3),
            "gone",
        ),
        TEXT,
        11,
    )?;
    store.delete(&gone, 12)?;
    assert!(store.thread(&gone).is_none());
    assert!(store.delete(&gone, 13).is_err(), "already gone");
    assert_eq!(store.threads().len(), 1);
    // A headless reply that raced the deletion lands after the
    // tombstone; the file still loads and the thread stays gone.
    let raced = serde_json::to_string(&Event::Reply {
        v: FORMAT_VERSION,
        thread: gone.clone(),
        reply: Reply::new(Author::agent("claude"), 14, "late"),
    })
    .map_err(|error| StoreError::parse(0, error.to_string()))?;
    let mut text = fs::read_to_string(&file.0).map_err(|e| StoreError::io(&file.0, e))?;
    text.push_str(&raced);
    text.push('\n');
    fs::write(&file.0, text).map_err(|e| StoreError::io(&file.0, e))?;
    let reloaded = Store::open(&file.0)?;
    assert!(reloaded.thread(&gone).is_none());
    assert!(reloaded.thread(&keep).is_some());
    Ok(())
}

#[test]
fn anchor_follows_moved_lines_and_detaches_when_gone() -> Result<(), String> {
    let range = LineRange::new(3, 4);
    let anchor = Anchor::capture(TEXT, range).ok_or("capture")?;
    assert_eq!(anchor.locate(TEXT, range), Some(range));
    let moved = "# Title\n\nintro\nmore intro\n\nalpha\nbeta\ngamma\n";
    assert_eq!(anchor.locate(moved, range), Some(LineRange::new(6, 7)));
    let edited = "# Title\n\nalpha\nBETA\ngamma\n";
    assert_eq!(anchor.locate(edited, range), None);
    assert_eq!(Anchor::capture(TEXT, LineRange::new(7, 9)), None);
    Ok(())
}

#[test]
fn relocate_moves_a_thread_and_the_user_acknowledges_the_edit() -> Result<(), StoreError> {
    let file = TempFile::new("relocate")?;
    let mut store = Store::open(&file.0)?;
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("README.md"),
            LineRange::new(3, 4),
            "rename",
        ),
        TEXT,
        100,
    )?;
    let edited = "# Title\n\nalpha\nBETA\ngamma\n";
    store.relocate(&id, LineRange::new(3, 4), edited, 110)?;
    let again = Store::open(&file.0)?;
    assert_eq!(again.threads(), store.threads());
    let thread = again
        .thread(&id)
        .ok_or_else(|| StoreError::parse(0, "lost".into()))?;
    assert!(thread.context().is_some());
    assert_eq!(thread.edited(), Some(110));
    assert_eq!(thread.updated(), 110);
    assert_eq!(
        thread.snippet(),
        "alpha\nbeta",
        "the snippet stays as commented on"
    );
    assert!(thread.locate(edited).is_edited());
    assert!(thread.locate(TEXT).is_detached());
    // An agent's reply leaves the edit flag; the user's clears it.
    store.reply(&id, Reply::new(Author::agent("claude"), 111, "fixed"))?;
    assert!(store.thread(&id).is_some_and(|t| t.edited().is_some()));
    store.reply(&id, Reply::new(Author::User, 112, "ok"))?;
    assert!(store.thread(&id).is_some_and(|t| t.edited().is_none()));
    assert!(
        store
            .thread(&id)
            .is_some_and(|t| !t.locate(edited).is_edited())
    );
    // A bad range is an error and writes nothing.
    assert!(
        store
            .relocate(&id, LineRange::new(8, 9), edited, 113)
            .is_err()
    );
    Ok(())
}

#[test]
fn move_path_carries_a_thread_to_the_renamed_file() -> Result<(), StoreError> {
    let file = TempFile::new("move")?;
    let mut store = Store::open(&file.0)?;
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("old.md"),
            LineRange::new(3, 4),
            "rename",
        ),
        TEXT,
        100,
    )?;
    store.move_path(&id, Path::new("docs/new.md"), 120)?;
    let again = Store::open(&file.0)?;
    assert_eq!(again.threads(), store.threads());
    let thread = again
        .thread(&id)
        .ok_or_else(|| StoreError::parse(0, "lost".into()))?;
    assert_eq!(thread.path(), Path::new("docs/new.md"));
    assert_eq!(thread.updated(), 120, "since polling sees the move");
    assert_eq!(thread.range(), Some(LineRange::new(3, 4)));
    assert_eq!(
        thread.locate(TEXT).range(),
        Some(LineRange::new(3, 4)),
        "the anchor still finds its lines"
    );
    assert!(again.for_path(Path::new("old.md")).next().is_none());
    let log = fs::read_to_string(&file.0).map_err(|e| StoreError::io(&file.0, e))?;
    assert!(log.contains(r#""event":"move""#));
    let unknown = ThreadId("nope".to_owned());
    assert!(store.move_path(&unknown, Path::new("x"), 1).is_err());
    Ok(())
}

#[test]
fn anchor_prefers_matching_context_then_nearest() -> Result<(), String> {
    // Two identical "item" lines; context picks the one after "two".
    let text = "one\nitem\ntwo\nitem\nthree\n";
    let anchor = Anchor::capture(text, LineRange::new(4, 4)).ok_or("capture")?;
    let shifted = "zero\none\nitem\ntwo\nitem\nthree\n";
    assert_eq!(
        anchor.locate(shifted, LineRange::new(4, 4)),
        Some(LineRange::new(5, 5))
    );
    // No context matches anywhere: nearest to the hint wins.
    let stripped = "item\nx\nitem\ny\nitem\n";
    assert_eq!(
        anchor.locate(stripped, LineRange::new(4, 4)),
        Some(LineRange::new(3, 3))
    );
    Ok(())
}

#[test]
fn authors_use_the_current_wire_shapes_and_labels() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(serde_json::to_string(&Author::User)?, r#""user""#);
    assert_eq!(serde_json::from_str::<Author>(r#""user""#)?, Author::User);
    let _ = serde_json::from_str::<Author>(r#""claude""#)
        .err()
        .ok_or("bare agent string was accepted")?;

    let plain = Author::agent("claude");
    let json = serde_json::to_string(&plain)?;
    assert_eq!(json, r#"{"name":"claude"}"#);
    assert_eq!(serde_json::from_str::<Author>(&json)?, plain);
    let _ = serde_json::from_str::<Author>(r#"{"name":"claude","kind":"coder"}"#)
        .err()
        .ok_or("retired agent kind was accepted")?;

    let full = Author::Agent {
        name: "reviewer".to_owned(),
        client: Some("claude-code".to_owned()),
        id: Some("chat-1".to_owned()),
    };
    let json = serde_json::to_string(&full)?;
    assert_eq!(
        json,
        r#"{"name":"reviewer","client":"claude-code","id":"chat-1"}"#
    );
    assert_eq!(serde_json::from_str::<Author>(&json)?, full);
    assert_eq!(full.to_string(), "reviewer (claude-code)");
    let same = Author::Agent {
        name: "claude-code".to_owned(),
        client: Some("claude-code".to_owned()),
        id: None,
    };
    assert_eq!(same.to_string(), "claude-code");
    assert_eq!(full.id(), Some("chat-1"));
    Ok(())
}

/// The last act identifies whether the next response is from an agent or
/// the user, while edits and reopens remain acts in their own right.
#[test]
fn the_last_act_decides_whose_turn_it_is() -> Result<(), StoreError> {
    let file = TempFile::new("pending")?;
    let mut store = Store::open(&file.0)?;
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(3, 3),
            "why?",
        ),
        TEXT,
        10,
    )?;
    let thread = |store: &Store| {
        store
            .thread(&id)
            .cloned()
            .ok_or(StoreError::parse(0, "gone".into()))
    };
    let t = thread(&store)?;
    assert!(t.awaits_agent() && !t.awaits_user());
    assert_eq!(t.last_act(), (&Author::User, 10));
    assert_eq!(t.newest(), (&Author::User, 10));

    // Any agent's reply is the answer; a second session does not owe one.
    let me = Author::agent("bot");
    store.reply(&id, Reply::new(me.clone(), 11, "because"))?;
    let t = thread(&store)?;
    assert!(!t.awaits_agent() && t.awaits_user());
    assert_eq!(t.last_act(), (&me, 11));

    // An edit of the comment, older than the reply, is the user's word.
    store.edit(&id, MessageTarget::Comment, "why, exactly?", 12)?;
    let t = thread(&store)?;
    assert!(t.awaits_agent());
    assert_eq!(t.last_act(), (&Author::User, 12));
    assert_eq!(t.comment_edited(), Some(12));
    assert_eq!(t.newest(), (&me, 11), "an edit is not a new message");

    store.reply(&id, Reply::new(me.clone(), 13, "ah"))?;
    store.reply(&id, Reply::new(Author::User, 14, "still"))?;
    store.edit(&id, MessageTarget::Reply(2), "still, yes", 15)?;
    let t = thread(&store)?;
    assert_eq!(t.replies()[2].edited(), Some(15));
    assert_eq!(t.last_act(), (&Author::User, 15));

    store.reply(
        &id,
        Reply::new(me.clone(), 16, "done").proposing_resolution(),
    )?;
    assert!(thread(&store)?.awaits_user());

    // Resolving ends both; reopening is the user's act, so the agent's turn.
    store.resolve(&id, None, 17)?;
    let t = thread(&store)?;
    assert!(!t.awaits_agent() && !t.awaits_user());
    store.reopen(&id, 18)?;
    let t = thread(&store)?;
    assert!(t.awaits_agent() && !t.awaits_user());
    assert_eq!(t.reopened(), Some(18));
    assert_eq!(t.last_act(), (&Author::User, 18));

    // At a tie a message beats a reopen.
    store.reply(&id, Reply::new(me.clone(), 18, "reopened, noted"))?;
    assert_eq!(thread(&store)?.last_act(), (&me, 18));
    assert!(thread(&store)?.awaits_user());

    let reloaded = Store::open(&file.0)?;
    assert_eq!(
        reloaded.thread(&id),
        store.thread(&id),
        "acts survive a reload"
    );
    Ok(())
}

/// An agent's comment is the agent's act, so its thread waits on the
/// user from birth; the record says who only for an agent, so a
/// record that says nothing loads as the user's.
#[test]
fn an_agents_comment_is_its_own_act() -> Result<(), StoreError> {
    let file = TempFile::new("agent-comment")?;
    let mut store = Store::open(&file.0)?;
    let bot = Author::agent("bot");
    let theirs = store.annotate(
        Draft::new(bot.clone(), Path::new("a.md"), LineRange::new(3, 3), "look"),
        TEXT,
        10,
    )?;
    let mine = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(4, 4),
            "why?",
        ),
        TEXT,
        11,
    )?;
    let reloaded = Store::open(&file.0)?;
    for store in [&store, &reloaded] {
        let t = store
            .thread(&theirs)
            .ok_or(StoreError::parse(0, "gone".into()))?;
        assert_eq!(t.author(), &bot);
        assert_eq!(t.last_act(), (&bot, 10));
        assert!(t.awaits_user() && !t.awaits_agent());
        let t = store
            .thread(&mine)
            .ok_or(StoreError::parse(0, "gone".into()))?;
        assert_eq!(t.author(), &Author::User);
        assert!(t.awaits_agent() && !t.awaits_user());
    }
    let lines: Vec<String> = std::fs::read_to_string(&file.0)
        .map_err(|e| StoreError::parse(0, e.to_string()))?
        .lines()
        .map(str::to_owned)
        .collect();
    assert!(lines[0].contains("\"author\""), "{}", lines[0]);
    assert!(!lines[1].contains("\"author\""), "{}", lines[1]);
    Ok(())
}

#[test]
fn two_handles_appending_to_one_file_keep_every_line_whole() -> Result<(), StoreError> {
    let file = TempFile::new("two-writers")?;
    let mut first = Store::open(&file.0)?;
    let id = first.annotate(
        Draft::new(
            Author::User,
            Path::new("README.md"),
            LineRange::new(3, 3),
            "one",
        ),
        TEXT,
        100,
    )?;
    // A second viewer, or a headless `--mcp` reply, opens its own handle.
    let mut second = Store::open(&file.0)?;
    for turn in 0..20 {
        first.reply(&id, Reply::new(Author::agent("a"), 200 + turn, "from a"))?;
        second.reply(&id, Reply::new(Author::agent("b"), 300 + turn, "from b"))?;
    }
    let text = fs::read_to_string(&file.0).map_err(|e| StoreError::io(&file.0, e))?;
    assert!(
        text.lines()
            .all(|line| line.starts_with('{') && line.ends_with('}'))
    );
    let merged = Store::open(&file.0)?;
    assert_eq!(merged.threads()[0].replies().len(), 40);
    Ok(())
}

#[test]
fn store_round_trips_threads_replies_and_status() -> Result<(), StoreError> {
    let file = TempFile::new("roundtrip")?;
    let mut store = Store::open(&file.0)?;
    let draft = Draft::new(
        Author::User,
        Path::new("README.md"),
        LineRange::new(3, 4),
        "rename",
    );
    let id = store.annotate(draft, TEXT, 100)?;
    store.reply(
        &id,
        Reply::new(Author::agent("claude"), 101, "done").proposing_resolution(),
    )?;
    let other = store.annotate(
        Draft::new(
            Author::User,
            Path::new("docs/guide.md"),
            LineRange::new(1, 1),
            "hmm",
        ),
        TEXT,
        102,
    )?;
    store.resolve(&other, None, 103)?;
    store.resolve(&id, None, 104)?;
    store.reopen(&id, 105)?;

    let again = Store::open(&file.0)?;
    assert_eq!(again.threads(), store.threads());
    let thread = again
        .thread(&id)
        .ok_or_else(|| StoreError::parse(0, "lost".into()))?;
    assert_eq!(thread.snippet(), "alpha\nbeta");
    assert!(thread.context().is_some());
    assert_eq!(thread.comment(), "rename");
    assert_eq!(thread.status(), Status::Open);
    assert_eq!(thread.replies().len(), 1);
    assert!(thread.replies()[0].proposes_resolution());
    assert!(thread.proposes_resolution());
    assert_eq!(thread.replies()[0].author().to_string(), "claude");
    assert_eq!(thread.updated(), 105);
    assert_eq!(
        again.thread(&other).map(super::Thread::status),
        Some(Status::Resolved)
    );
    assert_eq!(again.thread(&other).map(super::Thread::updated), Some(103));
    assert_eq!(again.for_path(Path::new("README.md")).count(), 1);

    let raw = fs::read_to_string(&file.0).map_err(|e| StoreError::io(&file.0, e))?;
    assert_eq!(raw.lines().count(), 6);
    assert!(raw.lines().all(|line| line.contains("\"v\":2")));
    assert!(raw.contains(r#""author":{"name":"claude"}"#));
    Ok(())
}

/// A thread carries the commit it was written against; the scope hides
/// it where that commit is not reachable, and a record with no commit
/// is shown everywhere (ADR 0024).
#[test]
fn threads_are_scoped_by_the_commit_they_were_written_against() -> Result<(), StoreError> {
    let file = TempFile::new("scope")?;
    let mut store = Store::open(&file.0)?;
    let unscoped = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(3, 3),
            "unscoped",
        )
        .at_commit(None),
        TEXT,
        1,
    )?;
    let scoped = store.annotate(
        Draft::new(Author::User, Path::new("a.md"), LineRange::new(2, 2), "new")
            .at_commit(Some("abc123".to_owned())),
        TEXT,
        2,
    )?;
    assert_eq!(store.thread(&unscoped).and_then(Thread::commit), None);
    assert_eq!(store.commits().collect::<Vec<_>>(), ["abc123"]);
    let again = Store::open(&file.0)?;
    assert_eq!(
        again.thread(&scoped).and_then(Thread::commit),
        Some("abc123")
    );

    let everywhere = Reach::everything();
    let on_branch = Reach::at("abc123", HashSet::from(["abc123".to_owned()]));
    let elsewhere = Reach::at("fff", HashSet::new());
    let visible = |scope: &Reach| -> Vec<&ThreadId> {
        again
            .threads()
            .iter()
            .filter(|t| scope.includes(t))
            .map(Thread::id)
            .collect()
    };
    assert_eq!(visible(&everywhere), [&unscoped, &scoped]);
    assert_eq!(visible(&on_branch), [&unscoped, &scoped]);
    assert_eq!(visible(&elsewhere), [&unscoped]);
    Ok(())
}

/// A rescope moves the thread to another commit and bumps `updated`,
/// and survives a reload (ADR 0035).
#[test]
fn a_rescope_moves_the_thread_to_the_new_commit() -> Result<(), StoreError> {
    let file = TempFile::new("rescope")?;
    let mut store = Store::open(&file.0)?;
    let id = store.annotate(
        Draft::new(Author::User, Path::new("a.md"), LineRange::new(1, 1), "hm")
            .at_commit(Some("old".to_owned())),
        TEXT,
        10,
    )?;
    store.rescope(&id, "new", 20)?;
    let again = Store::open(&file.0)?;
    let thread = again.thread(&id).ok_or(StoreError {
        kind: super::ErrorKind::UnknownThread(id.clone()),
    })?;
    assert_eq!(thread.commit(), Some("new"));
    assert_eq!(thread.updated(), 20);
    assert_eq!(thread.status(), Status::Open);
    assert!(Reach::at("new", HashSet::from(["new".to_owned()])).includes(thread));
    Ok(())
}

/// Resolving against a `HEAD` the thread is not at moves it there
/// first, so it shows at that commit alone; resolving at its own
/// commit, or outside git, appends nothing else (ADR 0072).
#[test]
fn resolving_fixes_the_thread_to_head() -> Result<(), Box<dyn std::error::Error>> {
    let file = TempFile::new("resolve-at")?;
    let mut store = Store::open(&file.0)?;
    let draft = |comment: &str, commit: Option<&str>| {
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            comment,
        )
        .at_commit(commit.map(str::to_owned))
    };
    let moved = store.annotate(draft("moved", Some("old")), TEXT, 10)?;
    let stays = store.annotate(draft("stays", Some("head")), TEXT, 11)?;
    let unscoped = store.annotate(draft("unscoped", None), TEXT, 12)?;
    let no_git = store.annotate(draft("no git", None), TEXT, 13)?;
    store.resolve(&moved, Some("head"), 20)?;
    store.resolve(&stays, Some("head"), 21)?;
    store.resolve(&unscoped, Some("head"), 22)?;
    store.resolve(&no_git, None, 23)?;
    let lines = std::fs::read_to_string(&file.0)?.lines().count();
    // Four comments, four resolves, and a rescope for the two that
    // were not at `head`.
    assert_eq!(lines, 10);

    let again = Store::open(&file.0)?;
    let commit_of = |id: &ThreadId| again.thread(id).and_then(Thread::commit);
    assert_eq!(commit_of(&moved), Some("head"));
    assert_eq!(commit_of(&stays), Some("head"));
    assert_eq!(commit_of(&unscoped), Some("head"));
    assert_eq!(commit_of(&no_git), None);
    assert!(
        again
            .threads()
            .iter()
            .all(|thread| thread.status() == Status::Resolved)
    );
    let at_head = Reach::at("head", HashSet::from(["head".to_owned(), "old".to_owned()]));
    let later = Reach::at("next", HashSet::from(["head".to_owned(), "old".to_owned()]));
    for id in [&moved, &stays, &unscoped] {
        let thread = again.thread(id).ok_or("thread missing")?;
        assert!(at_head.here(thread) && !later.here(thread) && later.past(thread));
    }
    Ok(())
}

#[test]
fn store_rejects_bad_ranges_unknown_threads_and_bad_lines() -> Result<(), StoreError> {
    let file = TempFile::new("errors")?;
    let mut store = Store::open(&file.0)?;
    let draft = Draft::new(Author::User, Path::new("a.md"), LineRange::new(9, 9), "x");
    let range_error = store.annotate(draft, TEXT, 1).err();
    assert!(range_error.is_some_and(|e| e.to_string().contains('9')));
    let ghost = super::ThreadId("nope".to_owned());
    let ghost_error = store.reopen(&ghost, 1).err();
    assert!(ghost_error.is_some_and(|e| e.to_string().contains("nope")));
    assert!(!file.0.exists(), "invalid events are never written");

    if let Some(parent) = file.0.parent() {
        fs::create_dir_all(parent).map_err(|e| StoreError::io(parent, e))?;
    }
    fs::write(&file.0, "{\"event\":\"dance\"}\n").map_err(|e| StoreError::io(&file.0, e))?;
    let Err(error) = Store::open(&file.0) else {
        return Err(StoreError::parse(0, "accepted garbage".into()));
    };
    assert!(
        error.to_string().starts_with("threads.jsonl line 1:"),
        "{error}"
    );
    assert!(!error.is_io());
    Ok(())
}

#[test]
fn thread_locate_reports_placement() -> Result<(), StoreError> {
    let file = TempFile::new("placement")?;
    let mut store = Store::open(&file.0)?;
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(5, 5),
            "gamma?",
        ),
        TEXT,
        1,
    )?;
    let thread = store.thread(&id).cloned();
    let thread = thread.ok_or_else(|| StoreError::parse(0, "lost".into()))?;
    assert_eq!(
        thread.locate(TEXT),
        Placement::Anchored(LineRange::new(5, 5))
    );
    let placement = thread.locate("nothing here\n");
    assert!(placement.is_detached());
    assert_eq!(placement.range(), Some(LineRange::new(5, 5)));
    Ok(())
}
