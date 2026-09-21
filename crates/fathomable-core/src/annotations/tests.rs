use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use fathomable_testing::TempDir;

use crate::reach::Reach;

use super::{
    ActivityCursor, AgentReplyCommand, Anchor, ArchiveContext, Author, AutoResolve,
    ComparisonFacts, Draft, Event, FORMAT_VERSION, FullFileDigest, LandingOutcome, Lifecycle,
    LineHashes, LineRange, MAX_IDEMPOTENCY_KEY_BYTES, MAX_MESSAGE_BYTES, MessageTarget, Origin,
    OriginSide, OriginVersion, Placement, PlacementContext, PlacementEvidence, Provenance, Reply,
    ResolutionOutcome, Status, Store, StoreError, Thread, ThreadId, UserSubmit, UserWriteOutcome,
    WorkingTreeFacts, WorkingTreeState, line_hash, start_intent,
};
use crate::context::Context;
use crate::workspace::CheckoutIdentity;

const TEXT: &str = "# Title\n\nalpha\nbeta\ngamma\n\ndelta\n";

#[test]
fn attaching_provenance_preserves_context_truncation() -> Result<(), &'static str> {
    let context = Context::capture_bounded(
        &format!("{}\n", "long".repeat(100)),
        LineRange::new(1, 1),
        12,
    )
    .ok_or("context")?;
    assert!(context.is_truncated());

    let origin = Origin::new(
        Path::new("src/lib.rs"),
        Some(LineRange::new(1, 1)),
        "long",
        None,
        Some(context),
    )
    .with_provenance(Provenance::new(
        OriginVersion::commit("0123456789abcdef"),
        OriginSide::Target,
    ));

    assert!(origin.evidence_truncated());
    Ok(())
}

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

#[test]
fn new_thread_messages_are_limited_by_utf8_bytes() -> Result<(), StoreError> {
    let file = TempFile::new("message-size")?;
    let mut store = Store::open(&file.0)?;
    let exact = "é".repeat(MAX_MESSAGE_BYTES / 2);
    let too_large = format!("{exact}é");
    let draft = |comment: String| {
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(3, 3),
            comment,
        )
    };

    let id = store.annotate(draft(exact.clone()), TEXT, 1)?;
    let start = store.annotate(draft(too_large.clone()), TEXT, 2);
    assert_eq!(
        start.err().map(|error| error.to_string()),
        Some(format!(
            "thread message has {} UTF-8 bytes; maximum is {MAX_MESSAGE_BYTES}",
            too_large.len()
        ))
    );
    assert_eq!(store.threads().len(), 1);

    let reply = store.reply(
        &id,
        Reply::new(Author::agent("reviewer"), 3, too_large.clone()),
    );
    assert!(reply.is_err());
    assert!(
        store
            .thread(&id)
            .is_some_and(|thread| thread.replies().is_empty())
    );

    let edit = store.edit_user(
        &id,
        MessageTarget::Comment,
        too_large.clone(),
        4,
        UserSubmit::Normal,
    );
    assert!(edit.is_err());
    assert_eq!(store.thread(&id).map(Thread::comment), Some(exact.as_str()));

    drop(store);
    let persisted = fs::read_to_string(&file.0).map_err(|e| StoreError::io(&file.0, e))?;
    fs::write(&file.0, persisted.replace(&exact, &too_large))
        .map_err(|e| StoreError::io(&file.0, e))?;
    assert_eq!(
        Store::open(&file.0)?.threads().first().map(Thread::comment),
        Some(too_large.as_str()),
        "messages written by an older build remain readable"
    );
    Ok(())
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
    let stale = current.replace(&format!(r#""v":{FORMAT_VERSION}"#), r#""v":7"#);
    fs::write(&file.0, stale).map_err(|e| StoreError::io(&file.0, e))?;
    let store_error = Store::open(&file.0).err();
    let mismatch = store_error.as_ref().and_then(StoreError::format_mismatch);
    assert_eq!(mismatch.as_ref().map(super::FormatMismatch::found), Some(7));
    assert_eq!(
        mismatch.as_ref().map(super::FormatMismatch::expected),
        Some(FORMAT_VERSION)
    );
    let error = store_error.map(|error| error.to_string());
    assert_eq!(
        error,
        Some(format!(
            "threads.jsonl line 1: format version 7, this build writes {FORMAT_VERSION}; delete {} to start over",
            file.0.display()
        ))
    );
    fs::write(&file.0, current).map_err(|e| StoreError::io(&file.0, e))?;
    assert_eq!(Store::open(&file.0)?.threads().len(), 1);
    Ok(())
}

fn working_tree_draft(store_path: &Path, text: &str) -> Draft {
    let checkout = store_path.parent().unwrap_or(Path::new("/")).to_path_buf();
    let repository = checkout.join(".git");
    Draft::new(
        Author::User,
        Path::new("a.md"),
        LineRange::new(1, 1),
        "mutable source",
    )
    .with_provenance(
        Provenance::new(
            OriginVersion::working_tree(Some("base".to_owned())),
            OriginSide::Target,
        )
        .with_working_tree(WorkingTreeFacts::new(
            Some("base".to_owned()),
            WorkingTreeState::Modified,
            Some(super::ContentIdentity::from_text(text)),
            CheckoutIdentity::from_canonical_paths(checkout, repository),
            FullFileDigest::from_bytes(text.as_bytes()),
        )),
    )
}

#[test]
fn mutable_origins_require_valid_full_provenance_on_create_and_load()
-> Result<(), Box<dyn std::error::Error>> {
    let file = TempFile::new("required-provenance")?;
    let mut store = Store::open(&file.0)?;
    for version in [
        OriginVersion::working_tree(Some("base".to_owned())),
        OriginVersion::index(Some("base".to_owned())),
        OriginVersion::review_point("point", Some("base".to_owned())),
    ] {
        let missing = Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "missing",
        )
        .with_provenance(Provenance::new(version, OriginSide::Target));
        assert!(
            store
                .annotate(missing, TEXT, 1)
                .is_err_and(|error| error.to_string().contains("full SHA-256"))
        );
    }

    store.annotate(working_tree_draft(&file.0, TEXT), TEXT, 2)?;
    let original = fs::read_to_string(&file.0)?;
    for field in ["checkout", "full_content"] {
        let mut event: serde_json::Value = serde_json::from_str(original.trim_end())?;
        event["origin"]["provenance"]["working_tree"]
            .as_object_mut()
            .ok_or("missing working-tree facts")?
            .remove(field);
        fs::write(&file.0, format!("{event}\n"))?;
        assert!(
            Store::open(&file.0).is_err(),
            "missing {field} was accepted"
        );
    }
    let mut malformed: serde_json::Value = serde_json::from_str(original.trim_end())?;
    malformed["origin"]["provenance"]["working_tree"]["full_content"]["sha256"] =
        serde_json::Value::String("ABC".to_owned());
    fs::write(&file.0, format!("{malformed}\n"))?;
    Store::open(&file.0)
        .err()
        .ok_or("malformed full digest was accepted")?;
    Ok(())
}

#[test]
fn mutable_comparisons_require_a_valid_checkout_qualifier() -> Result<(), Box<dyn std::error::Error>>
{
    let file = TempFile::new("comparison-checkout")?;
    let checkout = CheckoutIdentity::from_canonical_paths(
        file.0.clone(),
        file.0.parent().unwrap_or(Path::new("/")).to_path_buf(),
    );
    Store::open(&file.0)?.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "deletion",
        )
        .at_source(OriginVersion::commit("base"), OriginSide::Base)
        .in_comparison(ComparisonFacts::at_checkout(
            OriginVersion::commit("base"),
            OriginVersion::working_tree(Some("base".to_owned())),
            checkout,
        )),
        TEXT,
        1,
    )?;
    let original = fs::read_to_string(&file.0)?;
    let mut event: serde_json::Value = serde_json::from_str(original.trim_end())?;
    let removed = event["origin"]["provenance"]["comparison"]
        .as_object_mut()
        .ok_or("missing comparison facts")?
        .remove("checkout");
    assert!(removed.is_some(), "comparison checkout was not persisted");
    fs::write(&file.0, format!("{event}\n"))?;

    let error = Store::open(&file.0)
        .err()
        .ok_or("mutable comparison without checkout was accepted")?;
    assert!(
        error
            .to_string()
            .contains("comparisons require a checkout exactly when")
    );
    Ok(())
}

#[test]
fn full_file_digest_preserves_the_standard_sha256_encoding() {
    let digest = FullFileDigest::from_bytes(b"abc");
    assert_eq!(
        digest.sha256(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(digest.bytes(), 3);
}

#[test]
fn full_file_digest_covers_every_byte_and_length() {
    let base = FullFileDigest::from_bytes(b"same prefix, ending A");
    let changed = FullFileDigest::from_bytes(b"same prefix, ending B");
    let longer = FullFileDigest::from_bytes(b"same prefix, ending A!");

    assert_eq!(base.sha256().len(), 64);
    assert!(base.sha256().bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(base.bytes(), 21);
    assert_ne!(base, changed);
    assert_ne!(base, longer);
}

#[test]
fn landing_is_durable_first_writer_wins_without_touching_thread_metadata()
-> Result<(), Box<dyn std::error::Error>> {
    let file = TempFile::new("landing")?;
    let mut store = Store::open(&file.0)?;
    let id = store.annotate(working_tree_draft(&file.0, TEXT), TEXT, 10)?;
    store.archive(&id, 11)?;
    let before = store.thread(&id).ok_or("missing archived thread")?.clone();
    let candidate = before.landing_candidate().ok_or("missing candidate")?;
    let activity_before = store
        .agent_activity_since(ActivityCursor::default())
        .count();
    let first = crate::workspace::CommitId::parse("1111111111111111111111111111111111111111")?;
    let other = crate::workspace::CommitId::parse("2222222222222222222222222222222222222222")?;

    assert_eq!(store.land(&candidate, &first)?, LandingOutcome::Applied);
    let landed = store.thread(&id).ok_or("missing landed thread")?;
    assert_eq!(landed.landed_commit(), Some(first.as_str()));
    assert_eq!(landed.modified(), before.modified());
    assert_eq!(landed.lifecycle(), before.lifecycle());
    assert_eq!(landed.auto_resolve(), before.auto_resolve());
    assert_eq!(landed.revision(), before.revision());
    assert_eq!(landed.commit(), before.commit());
    assert!(landed.is_archived());
    assert_eq!(
        store
            .agent_activity_since(ActivityCursor::default())
            .count(),
        activity_before
    );
    let lines = fs::read_to_string(&file.0)?.lines().count();

    drop(store);
    let mut retry = Store::open(&file.0)?;
    assert_eq!(
        retry.land(&candidate, &first)?,
        LandingOutcome::AlreadyLanded
    );
    assert_eq!(
        retry.land(&candidate, &other)?,
        LandingOutcome::Conflict {
            landed_commit: first.as_str().to_owned()
        }
    );
    assert_eq!(fs::read_to_string(&file.0)?.lines().count(), lines);
    assert_eq!(
        Store::open(&file.0)?
            .thread(&id)
            .and_then(Thread::landed_commit),
        Some(first.as_str())
    );
    Ok(())
}

#[test]
fn landing_rechecks_concurrent_deletion_and_first_writer_under_lock()
-> Result<(), Box<dyn std::error::Error>> {
    let file = TempFile::new("landing-race")?;
    let mut initial = Store::open(&file.0)?;
    let deleted_id = initial.annotate(working_tree_draft(&file.0, TEXT), TEXT, 1)?;
    let deleted_candidate = initial
        .thread(&deleted_id)
        .and_then(Thread::landing_candidate)
        .ok_or("missing deletion candidate")?;
    let winner_id = initial.annotate(working_tree_draft(&file.0, TEXT), TEXT, 2)?;
    let winner_candidate = initial
        .thread(&winner_id)
        .and_then(Thread::landing_candidate)
        .ok_or("missing winner candidate")?;
    let mut stale = Store::open(&file.0)?;
    initial.delete(&deleted_id, 3)?;
    let first = crate::workspace::CommitId::parse("3333333333333333333333333333333333333333")?;
    let second = crate::workspace::CommitId::parse("4444444444444444444444444444444444444444")?;
    assert_eq!(
        stale.land(&deleted_candidate, &first)?,
        LandingOutcome::Deleted
    );

    let mut competing = Store::open(&file.0)?;
    assert_eq!(
        stale.land(&winner_candidate, &first)?,
        LandingOutcome::Applied
    );
    assert_eq!(
        competing.land(&winner_candidate, &second)?,
        LandingOutcome::Conflict {
            landed_commit: first.as_str().to_owned()
        }
    );
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
    assert_eq!(thread.modified(), 14);
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
    let moved = store.relocate(
        &id,
        LineRange::new(2, 2),
        TEXT,
        PlacementContext::new(OriginVersion::working_tree(None)),
        11,
    );
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
        submission: UserSubmit::Normal,
        relocation: None,
        receipt: None,
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
fn relocate_moves_a_thread_and_keeps_the_reanchor_fact() -> Result<(), StoreError> {
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
    store.relocate(
        &id,
        LineRange::new(3, 4),
        edited,
        PlacementContext::new(OriginVersion::working_tree(Some("head".to_owned())))
            .at_checkout("/checkout"),
        110,
    )?;
    let again = Store::open(&file.0)?;
    assert_eq!(again.threads(), store.threads());
    let thread = again
        .thread(&id)
        .ok_or_else(|| StoreError::parse(0, "lost".into()))?;
    assert!(thread.context().is_some());
    assert_eq!(thread.reanchored_at(), Some(110));
    assert_eq!(thread.modified(), 110);
    assert_eq!(
        thread.placement_evidence().version(),
        &OriginVersion::working_tree(Some("head".to_owned()))
    );
    assert_eq!(thread.placement_evidence().checkout(), Some("/checkout"));
    assert_eq!(
        thread.snippet(),
        "alpha\nbeta",
        "the snippet stays as commented on"
    );
    assert!(thread.locate(edited).is_edited());
    assert!(thread.locate(TEXT).is_detached());
    // Replies preserve the factual re-anchor time.
    store.reply(&id, Reply::new(Author::agent("claude"), 111, "fixed"))?;
    assert!(
        store
            .thread(&id)
            .is_some_and(|t| t.reanchored_at().is_some())
    );
    store.reply(&id, Reply::new(Author::User, 112, "ok"))?;
    assert_eq!(store.thread(&id).and_then(Thread::reanchored_at), Some(110));
    assert!(
        store
            .thread(&id)
            .is_some_and(|t| t.locate(edited).is_edited())
    );
    store.resolve(&id, Some("resolved-head"), 112)?;
    assert_eq!(
        store
            .thread(&id)
            .map(Thread::placement_evidence)
            .map(PlacementEvidence::version),
        Some(&OriginVersion::working_tree(Some("head".to_owned()))),
        "resolution context does not retag placement evidence"
    );
    // A bad range is an error and writes nothing.
    assert!(
        store
            .relocate(
                &id,
                LineRange::new(8, 9),
                edited,
                PlacementContext::new(OriginVersion::working_tree(None)),
                113,
            )
            .is_err()
    );
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
    assert_eq!(thread.lifecycle(), Lifecycle::Active);
    assert_eq!(thread.replies()[0].author().to_string(), "claude");
    assert_eq!(thread.modified(), 105);
    assert_eq!(
        again.thread(&other).map(super::Thread::status),
        Some(Status::Resolved)
    );
    assert_eq!(again.thread(&other).map(super::Thread::modified), Some(103));
    assert_eq!(again.for_path(Path::new("README.md")).count(), 1);

    let raw = fs::read_to_string(&file.0).map_err(|e| StoreError::io(&file.0, e))?;
    assert_eq!(raw.lines().count(), 6);
    assert!(
        raw.lines()
            .all(|line| line.contains(&format!("\"v\":{FORMAT_VERSION}")))
    );
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
    assert_eq!(visible(&elsewhere), [&unscoped, &scoped]);
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
    // Four comments and four atomic resolve-plus-pin events.
    assert_eq!(lines, 8);

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
    crate::private_state::write(&file.0, "{\"event\":\"dance\"}\n")
        .map_err(|e| StoreError::io(&file.0, e))?;
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

fn keyed_author(id: &str) -> Author {
    Author::Agent {
        name: "reviewer".to_owned(),
        client: Some("copilot-cli".to_owned()),
        id: Some(id.to_owned()),
    }
}

#[test]
fn keyed_writes_require_a_valid_key_and_authenticated_caller() -> Result<(), StoreError> {
    let file = TempFile::new("idempotent-validation")?;
    let draft = Draft::on_file(keyed_author("copilot:one"), Path::new("a.md"), "comment");
    let mut store = Store::open(&file.0)?;
    for key in ["", " \t"] {
        let result = store.annotate_idempotent(draft.clone(), 1, key, |_| {
            Err(StoreError::message("invalid key was loaded"))
        });
        assert!(
            result
                .as_ref()
                .is_err_and(|error| error.to_string().contains("idempotency key")),
            "{result:?}"
        );
    }
    let too_long = "x".repeat(MAX_IDEMPOTENCY_KEY_BYTES + 1);
    let result = store.annotate_idempotent(draft, 1, &too_long, |_| {
        Err(StoreError::message("oversized key was loaded"))
    });
    assert!(
        result
            .as_ref()
            .is_err_and(|error| error.to_string().contains("at most")),
        "{result:?}"
    );

    let unauthenticated = Draft::on_file(Author::agent("reviewer"), Path::new("a.md"), "comment");
    let result = store.annotate_idempotent(unauthenticated, 1, "valid", |_| {
        Err(StoreError::message("unauthenticated caller was loaded"))
    });
    assert!(
        result
            .as_ref()
            .is_err_and(|error| error.to_string().contains("caller identity")),
        "{result:?}"
    );
    assert!(!file.0.exists(), "validation must not create the store");
    Ok(())
}

#[test]
fn keyed_start_replays_after_restart_without_loading_the_source() -> Result<(), StoreError> {
    let file = TempFile::new("idempotent-start")?;
    let author = keyed_author("copilot:one");
    let draft = Draft::new(
        author.clone(),
        Path::new("./nested/../a.md"),
        LineRange::new(4, 3),
        "same body",
    );
    let mut store = Store::open(&file.0)?;
    let first = store.annotate_idempotent(draft.clone(), 20, "start-1", |path| {
        assert_eq!(path, Path::new("a.md"));
        Ok(TEXT.to_owned())
    })?;
    assert!(!first.replayed());
    let id = first.into_value();
    let event: serde_json::Value = serde_json::from_str(
        fs::read_to_string(&file.0)
            .map_err(|error| StoreError::io(&file.0, error))?
            .trim_end(),
    )
    .map_err(|error| StoreError::parse(0, error.to_string()))?;
    assert_eq!(
        event["receipt"]["intent"].as_str(),
        Some(
            r#"{"operation":"start","path":"a.md","range":{"start":3,"end":4},"body":"same body"}"#
        ),
        "legacy format-5 receipt intent must remain byte-for-byte stable"
    );
    drop(store);

    let mut restarted = Store::open(&file.0)?;
    let replay = restarted.annotate_idempotent(draft, 99, "start-1", |_| {
        Err(StoreError::message("source must not be loaded on replay"))
    })?;
    assert!(replay.replayed());
    assert_eq!(replay.value(), &id);
    assert_eq!(restarted.threads().len(), 1);
    assert_eq!(restarted.thread(&id).map(Thread::created), Some(20));
    Ok(())
}

#[test]
fn selected_commit_intent_remains_tagged_in_current_format() -> Result<(), StoreError> {
    let commit = crate::workspace::CommitId::parse("0123456789abcdef0123456789abcdef01234567")
        .map_err(|error| StoreError::message(error.to_string()))?;
    let legacy = Draft::on_file(keyed_author("copilot:one"), Path::new("a.md"), "comment");
    let selected = legacy.clone().at_selected_commit(commit);
    assert_eq!(
        selected.provenance().version().commit_id(),
        Some("0123456789abcdef0123456789abcdef01234567")
    );
    assert_eq!(selected.provenance().side(), OriginSide::Unspecified);

    assert_eq!(
        start_intent(&legacy)?,
        r#"{"operation":"start","path":"a.md","range":null,"body":"comment"}"#
    );
    assert_eq!(
        start_intent(&selected)?,
        r#"{"operation":"start","path":"a.md","range":null,"body":"comment","source":{"kind":"commit","id":"0123456789abcdef0123456789abcdef01234567"}}"#
    );
    assert_eq!(FORMAT_VERSION, 8);
    Ok(())
}

#[test]
fn selected_commit_receipt_rejects_a_disagreeing_durable_origin() -> Result<(), StoreError> {
    let file = TempFile::new("selected-origin-mismatch")?;
    let selected = crate::workspace::CommitId::parse("1111111111111111111111111111111111111111")
        .map_err(|error| StoreError::message(error.to_string()))?;
    let draft = Draft::on_file(keyed_author("copilot:one"), Path::new("a.md"), "comment")
        .at_selected_commit(selected);
    Store::open(&file.0)?
        .annotate_idempotent(draft.clone(), 1, "selected", |_| Ok(String::new()))?;

    let mut event: serde_json::Value = serde_json::from_str(
        fs::read_to_string(&file.0)
            .map_err(|error| StoreError::io(&file.0, error))?
            .trim_end(),
    )
    .map_err(|error| StoreError::parse(0, error.to_string()))?;
    let other = serde_json::Value::String("2222222222222222222222222222222222222222".to_owned());
    event["origin"]["provenance"]["version"]["id"] = other.clone();
    event["commit"] = other;
    fs::write(&file.0, format!("{event}\n")).map_err(|error| StoreError::io(&file.0, error))?;

    let mut store = Store::open(&file.0)?;
    let conflict = store.probe_start_idempotency(&draft, "selected");
    assert!(
        conflict
            .as_ref()
            .is_err_and(|error| error.to_string().contains("conflicts")),
        "{conflict:?}"
    );
    let conflict = store.annotate_idempotent(draft, 2, "selected", |_| {
        Err(StoreError::message(
            "conflicting replay must not load source",
        ))
    });
    assert!(
        conflict
            .as_ref()
            .is_err_and(|error| error.to_string().contains("conflicts")),
        "{conflict:?}"
    );
    Ok(())
}

#[test]
fn idempotency_probes_replay_before_source_or_lifecycle_checks() -> Result<(), StoreError> {
    let file = TempFile::new("idempotent-probe")?;
    let author = keyed_author("copilot:one");
    let draft = Draft::on_file(author.clone(), Path::new("missing.md"), "comment");
    let mut store = Store::open(&file.0)?;
    let start_id = store
        .annotate_idempotent(draft.clone(), 1, "start-1", |_| Ok(String::new()))?
        .into_value();
    assert_eq!(
        store.probe_start_idempotency(&draft, "start-1")?,
        Some(start_id.clone())
    );
    let conflict = store.probe_start_idempotency(
        &Draft::on_file(author.clone(), Path::new("missing.md"), "different"),
        "start-1",
    );
    assert!(
        conflict
            .as_ref()
            .is_err_and(|error| error.to_string().contains("conflicts")),
        "{conflict:?}"
    );

    let reply = Reply::new(author, 2, "answer").proposing_resolution();
    store.reply_idempotent(&start_id, reply.clone(), None, "reply-1", |_| {
        Ok(String::new())
    })?;
    assert_eq!(
        store.probe_reply_idempotency(&start_id, &reply, None, "reply-1")?,
        Some(start_id.clone())
    );
    store.delete(&start_id, 3)?;
    let deleted = store.probe_reply_idempotency(&start_id, &reply, None, "reply-1");
    assert!(
        deleted
            .as_ref()
            .is_err_and(|error| error.to_string().contains("was deleted")),
        "{deleted:?}"
    );

    let explicit_draft = Draft::on_file(
        Author::agent("display-only"),
        Path::new("other.md"),
        "explicit",
    );
    let explicit_id = store
        .annotate_idempotent_for_caller(
            explicit_draft.clone(),
            4,
            "copilot:explicit",
            "explicit-1",
            |_| Ok(String::new()),
        )?
        .into_value();
    assert_eq!(
        store.probe_start_idempotency_for_caller(
            &explicit_draft,
            "copilot:explicit",
            "explicit-1"
        )?,
        Some(explicit_id.clone())
    );
    assert_eq!(
        store
            .thread(&explicit_id)
            .and_then(|thread| thread.author().id()),
        None
    );
    Ok(())
}

#[test]
fn keyed_start_conflicts_on_different_effective_arguments() -> Result<(), StoreError> {
    let file = TempFile::new("idempotent-conflict")?;
    let author = keyed_author("copilot:one");
    let mut store = Store::open(&file.0)?;
    let draft = Draft::on_file(author.clone(), Path::new("a.md"), "first");
    store.annotate_idempotent(draft, 1, "same", |_| Ok(TEXT.to_owned()))?;
    let conflict = store.annotate_idempotent(
        Draft::on_file(author, Path::new("a.md"), "second"),
        2,
        "same",
        |_| Ok(TEXT.to_owned()),
    );
    assert!(
        conflict
            .as_ref()
            .is_err_and(|error| error.to_string().contains("conflicts")),
        "{conflict:?}"
    );
    assert_eq!(store.threads().len(), 1);
    assert_eq!(
        std::fs::read_to_string(&file.0)
            .map_err(|error| StoreError::io(&file.0, error))?
            .lines()
            .count(),
        1
    );
    Ok(())
}

#[test]
fn keyed_start_is_concurrent_and_scoped_by_caller_and_operation() -> Result<(), StoreError> {
    let file = TempFile::new("idempotent-scope")?;
    let path = file.0.clone();
    let first_path = path.clone();
    let second_path = path.clone();
    let first = std::thread::spawn(move || -> Result<ThreadId, StoreError> {
        let mut store = Store::open(first_path)?;
        Ok(store
            .annotate_idempotent(
                Draft::on_file(keyed_author("copilot:one"), Path::new("a.md"), "one"),
                30,
                "same",
                |_| Ok(TEXT.to_owned()),
            )?
            .into_value())
    });
    let second = std::thread::spawn(move || -> Result<ThreadId, StoreError> {
        let mut store = Store::open(second_path)?;
        Ok(store
            .annotate_idempotent(
                Draft::on_file(keyed_author("copilot:one"), Path::new("a.md"), "one"),
                31,
                "same",
                |_| Ok(TEXT.to_owned()),
            )?
            .into_value())
    });
    let first = first
        .join()
        .map_err(|_panic| StoreError::message("thread panicked"))??;
    let second = second
        .join()
        .map_err(|_panic| StoreError::message("thread panicked"))??;
    assert_eq!(first, second);

    let mut store = Store::open(&path)?;
    let other_caller = store.annotate_idempotent(
        Draft::on_file(keyed_author("copilot:two"), Path::new("a.md"), "one"),
        32,
        "same",
        |_| Ok(TEXT.to_owned()),
    )?;
    assert!(!other_caller.replayed());
    let other_operation = store.reply_idempotent(
        &first,
        Reply::new(keyed_author("copilot:one"), 33, "answer"),
        None,
        "same",
        |_| Ok(TEXT.to_owned()),
    )?;
    assert!(!other_operation.replayed());
    assert_eq!(store.threads().len(), 2);
    assert_eq!(
        store.thread(&first).map(|thread| thread.replies().len()),
        Some(1)
    );
    Ok(())
}

#[test]
fn keyed_reply_replays_without_relocation_or_mutable_validation() -> Result<(), StoreError> {
    let file = TempFile::new("idempotent-reply")?;
    let mut store = Store::open(&file.0)?;
    let id = store.annotate(
        Draft::new(Author::User, Path::new("a.md"), LineRange::new(3, 3), "why"),
        TEXT,
        1,
    )?;
    let reply = Reply::new(keyed_author("copilot:one"), 2, "answer").proposing_resolution();
    let first = store.reply_idempotent(
        &id,
        reply.clone(),
        Some(LineRange::new(3, 4)),
        "reply-1",
        |path| {
            assert_eq!(path, Path::new("a.md"));
            Ok(TEXT.to_owned())
        },
    )?;
    assert!(!first.replayed());
    let before = store
        .thread(&id)
        .cloned()
        .ok_or_else(|| StoreError::message("thread missing after idempotent reply"))?;
    drop(store);

    let mut restarted = Store::open(&file.0)?;
    let replayed =
        restarted.reply_idempotent(&id, reply, Some(LineRange::new(4, 3)), "reply-1", |_| {
            Err(StoreError::message("source must not be loaded on replay"))
        })?;
    assert!(replayed.replayed());
    let after = restarted
        .thread(&id)
        .ok_or_else(|| StoreError::message("thread missing after replay"))?;
    assert_eq!(after.replies().len(), 1);
    assert_eq!(after.modified(), before.modified());
    assert_eq!(after.range(), Some(LineRange::new(3, 4)));
    Ok(())
}

#[test]
fn keyed_reply_at_the_current_range_preserves_the_anchor() -> Result<(), StoreError> {
    let file = TempFile::new("idempotent-current-range")?;
    let mut store = Store::open(&file.0)?;
    let range = LineRange::new(3, 4);
    let id = store.annotate(
        Draft::new(Author::User, Path::new("a.md"), range, "why"),
        TEXT,
        1,
    )?;

    store.reply_idempotent(
        &id,
        Reply::new(keyed_author("copilot:one"), 2, "answer"),
        Some(range),
        "reply-current",
        |_| Ok(TEXT.to_owned()),
    )?;

    let thread = store
        .thread(&id)
        .ok_or_else(|| StoreError::message("thread missing after reply"))?;
    assert_eq!(thread.locate(TEXT), Placement::Anchored(range));
    assert_eq!(thread.reanchored_at(), None);
    assert_eq!(thread.replies().len(), 1);
    Ok(())
}

#[test]
fn keyed_replay_of_deleted_thread_is_explicitly_rejected() -> Result<(), StoreError> {
    let file = TempFile::new("idempotent-deleted")?;
    let author = keyed_author("copilot:one");
    let mut store = Store::open(&file.0)?;
    let id = store
        .annotate_idempotent(
            Draft::on_file(author.clone(), Path::new("a.md"), "comment"),
            1,
            "start-1",
            |_| Ok(TEXT.to_owned()),
        )?
        .into_value();
    store.delete(&id, 2)?;
    let replay = store.annotate_idempotent(
        Draft::on_file(author, Path::new("a.md"), "comment"),
        3,
        "start-1",
        |_| Err(StoreError::message("source must not be loaded")),
    );
    assert!(
        replay
            .as_ref()
            .is_err_and(|error| error.to_string().contains("was deleted")),
        "{replay:?}"
    );
    Ok(())
}

#[test]
fn keyed_reply_rechecks_resolution_under_the_write_lock() -> Result<(), StoreError> {
    let file = TempFile::new("idempotent-resolved-race")?;
    let mut writer = Store::open(&file.0)?;
    let id = writer.annotate(
        Draft::on_file(Author::User, Path::new("a.md"), "question"),
        TEXT,
        1,
    )?;
    let mut stale = Store::open(&file.0)?;
    let answer = Reply::new(keyed_author("copilot:one"), 3, "answer");
    assert!(
        stale
            .probe_reply_idempotency(&id, &answer, None, "reply-1")?
            .is_none()
    );
    writer.resolve(&id, None, 2)?;
    let outcome = stale.reply_idempotent(&id, answer.clone(), None, "reply-1", |_| {
        Err(StoreError::message("file-wide reply must not load source"))
    });
    assert!(
        outcome
            .as_ref()
            .is_err_and(|error| error.to_string().contains("is resolved"))
    );
    assert_eq!(
        Store::open(&file.0)?
            .thread(&id)
            .map(|thread| thread.replies().len()),
        Some(0)
    );
    writer.reopen(&id, 4)?;
    let outcome = stale.reply_idempotent(&id, answer, None, "reply-1", |_| {
        Err(StoreError::message("file-wide reply must not load source"))
    })?;
    assert!(!outcome.replayed(), "rejected writes must not reserve keys");
    Ok(())
}

#[test]
fn failed_reload_preserves_the_last_complete_in_memory_store() -> Result<(), StoreError> {
    let file = TempFile::new("idempotent-bad-reload")?;
    let mut store = Store::open(&file.0)?;
    let draft = Draft::on_file(keyed_author("copilot:one"), Path::new("a.md"), "question");
    store.annotate_idempotent(draft.clone(), 1, "start-1", |_| Ok(TEXT.to_owned()))?;
    let before = store.threads().to_vec();
    fs::write(&file.0, "invalid event\n").map_err(|error| StoreError::io(&file.0, error))?;
    let result = store.annotate_idempotent(draft, 2, "start-1", |_| {
        Err(StoreError::message("corrupt store must not load source"))
    });
    assert!(
        result
            .as_ref()
            .is_err_and(|error| error.to_string().contains("line 1"))
    );
    assert_eq!(store.threads(), before);
    Ok(())
}

#[test]
fn deleting_a_keyed_thread_does_not_reuse_its_id() -> Result<(), StoreError> {
    let file = TempFile::new("idempotent-deleted-id")?;
    let author = keyed_author("copilot:one");
    let mut store = Store::open(&file.0)?;
    let draft = Draft::on_file(author.clone(), Path::new("a.md"), "comment");
    let original = store
        .annotate_idempotent(draft, 1, "start-1", |_| Ok(TEXT.to_owned()))?
        .into_value();
    store.delete(&original, 2)?;

    let replacement = store.annotate(
        Draft::on_file(author, Path::new("a.md"), "replacement"),
        TEXT,
        1,
    )?;
    assert_ne!(replacement, original);
    assert!(
        store.thread(&replacement).is_some(),
        "replacement thread should remain writable"
    );
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

#[test]
fn messages_project_opening_comment_and_replies_uniformly() -> Result<(), StoreError> {
    let file = TempFile::new("messages")?;
    let mut store = Store::open(&file.0)?;
    let bot = Author::agent("bot");
    let id = store.annotate(
        Draft::on_file(bot.clone(), Path::new("a.md"), "opening"),
        TEXT,
        10,
    )?;
    store.reply_user(&id, 11, "answer", UserSubmit::Normal)?;
    store.edit(&id, MessageTarget::Reply(0), "edited answer", 12)?;

    let thread = store
        .thread(&id)
        .ok_or_else(|| StoreError::message("thread missing"))?;
    let messages = thread.messages().collect::<Vec<_>>();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].target(), MessageTarget::Comment);
    assert_eq!(messages[0].author(), &bot);
    assert_eq!(messages[0].body(), "opening");
    assert_eq!((messages[0].created(), messages[0].modified()), (10, 10));
    assert!(!messages[0].resolution_proposed());
    assert_eq!(messages[1].target(), MessageTarget::Reply(0));
    assert_eq!(messages[1].body(), "edited answer");
    assert_eq!((messages[1].created(), messages[1].modified()), (11, 12));
    assert_eq!(thread.newest(), (&Author::User, 11));
    Ok(())
}

#[test]
fn historical_proposal_does_not_resurrect_after_reopen() -> Result<(), StoreError> {
    let file = TempFile::new("proposal-reopen")?;
    let mut store = Store::open(&file.0)?;
    let id = store.annotate(
        Draft::on_file(Author::User, Path::new("a.md"), "question"),
        TEXT,
        1,
    )?;
    let proposed = store.agent_reply(
        &id,
        AgentReplyCommand::new(Author::agent("bot"), 2, "done").resolve(),
        |_| Ok(TEXT.to_owned()),
    )?;
    assert_eq!(proposed.into_value(), ResolutionOutcome::ResolutionProposed);
    store.resolve(&id, Some("head"), 3)?;
    store.reopen(&id, 4)?;

    let thread = store
        .thread(&id)
        .ok_or_else(|| StoreError::message("thread missing"))?;
    assert_eq!(thread.lifecycle(), Lifecycle::Active);
    assert!(thread.replies()[0].proposes_resolution());
    assert!(
        thread
            .messages()
            .nth(1)
            .is_some_and(|m| m.resolution_proposed())
    );
    Ok(())
}

#[test]
fn ordinary_message_supersedes_current_proposal() -> Result<(), StoreError> {
    let file = TempFile::new("proposal-superseded")?;
    let mut store = Store::open(&file.0)?;
    let id = store.annotate(
        Draft::on_file(Author::User, Path::new("a.md"), "question"),
        TEXT,
        1,
    )?;
    store.agent_reply(
        &id,
        AgentReplyCommand::new(Author::agent("bot"), 2, "done").resolve(),
        |_| Ok(TEXT.to_owned()),
    )?;
    store.agent_reply(
        &id,
        AgentReplyCommand::new(Author::agent("bot"), 3, "one more thing"),
        |_| Ok(TEXT.to_owned()),
    )?;
    assert_eq!(
        store.thread(&id).map(Thread::lifecycle),
        Some(Lifecycle::Active)
    );
    store.agent_reply(
        &id,
        AgentReplyCommand::new(Author::agent("bot"), 4, "now done").resolve(),
        |_| Ok(TEXT.to_owned()),
    )?;
    store.reply_user(&id, 5, "not yet", UserSubmit::Normal)?;
    let thread = store
        .thread(&id)
        .ok_or_else(|| StoreError::message("thread missing"))?;
    assert_eq!(thread.lifecycle(), Lifecycle::Active);
    assert!(thread.replies()[0].proposes_resolution());
    Ok(())
}

#[test]
fn editing_an_older_message_preserves_current_proposal() -> Result<(), StoreError> {
    let file = TempFile::new("proposal-edit")?;
    let mut store = Store::open(&file.0)?;
    let id = store.annotate(
        Draft::on_file(Author::User, Path::new("a.md"), "question"),
        TEXT,
        1,
    )?;
    store.reply_user(&id, 2, "detail", UserSubmit::Normal)?;
    store.agent_reply(
        &id,
        AgentReplyCommand::new(Author::agent("bot"), 3, "done").resolve(),
        |_| Ok(TEXT.to_owned()),
    )?;
    store.edit(&id, MessageTarget::Comment, "clearer question", 4)?;
    assert_eq!(
        store.thread(&id).map(Thread::lifecycle),
        Some(Lifecycle::ResolutionProposed)
    );
    Ok(())
}

#[test]
fn ordinary_agent_reply_consumes_auto_resolve() -> Result<(), StoreError> {
    let file = TempFile::new("auto-consume")?;
    let mut store = Store::open(&file.0)?;
    let id = store.annotate(
        Draft::on_file(Author::User, Path::new("a.md"), "question"),
        TEXT,
        1,
    )?;
    store.set_auto_resolve(&id, AutoResolve::Enabled, 2)?;
    let outcome = store.agent_reply(
        &id,
        AgentReplyCommand::new(Author::agent("bot"), 3, "progress"),
        |_| Ok(TEXT.to_owned()),
    )?;
    assert_eq!(outcome.into_value(), ResolutionOutcome::NotRequested);
    let thread = store
        .thread(&id)
        .ok_or_else(|| StoreError::message("thread missing"))?;
    assert_eq!(thread.auto_resolve(), AutoResolve::Disabled);
    assert_eq!(thread.lifecycle(), Lifecycle::Active);
    Ok(())
}

#[test]
fn authorized_resolving_agent_reply_is_one_atomic_event() -> Result<(), StoreError> {
    let file = TempFile::new("agent-resolve")?;
    let mut store = Store::open(&file.0)?;
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(3, 3),
            "question",
        )
        .at_commit(Some("old".to_owned())),
        TEXT,
        1,
    )?;
    store.set_auto_resolve(&id, AutoResolve::Enabled, 2)?;
    let before = fs::read_to_string(&file.0)
        .map_err(|error| StoreError::io(&file.0, error))?
        .lines()
        .count();
    let outcome = store.agent_reply(
        &id,
        AgentReplyCommand::new(Author::agent("bot"), 3, "fixed")
            .resolve()
            .relocate(LineRange::new(3, 4))
            .at_head(Some("head".to_owned())),
        |_| Ok(TEXT.to_owned()),
    )?;
    assert_eq!(outcome.into_value(), ResolutionOutcome::Resolved);
    let raw = fs::read_to_string(&file.0).map_err(|error| StoreError::io(&file.0, error))?;
    assert_eq!(raw.lines().count(), before + 1);
    assert!(raw.lines().last().is_some_and(|line| {
        line.contains(r#""event":"agent_reply""#)
            && line.contains(r#""resolution":"resolved""#)
            && line.contains(r#""commit":"head""#)
            && line.contains(r#""relocation""#)
    }));
    let thread = store
        .thread(&id)
        .ok_or_else(|| StoreError::message("thread missing"))?;
    assert_eq!(thread.lifecycle(), Lifecycle::Resolved);
    assert_eq!(thread.auto_resolve(), AutoResolve::Disabled);
    assert_eq!(thread.commit(), Some("head"));
    assert_eq!(thread.range(), Some(LineRange::new(3, 4)));
    assert_eq!(thread.reanchored_at(), Some(3));
    Ok(())
}

#[test]
fn unauthorized_resolving_agent_reply_records_a_proposal() -> Result<(), StoreError> {
    let file = TempFile::new("agent-propose")?;
    let mut store = Store::open(&file.0)?;
    let id = store.annotate(
        Draft::on_file(Author::User, Path::new("a.md"), "question"),
        TEXT,
        1,
    )?;
    let outcome = store.agent_reply(
        &id,
        AgentReplyCommand::new(Author::agent("bot"), 2, "fixed").resolve(),
        |_| Ok(TEXT.to_owned()),
    )?;
    assert_eq!(outcome.into_value(), ResolutionOutcome::ResolutionProposed);
    let thread = store
        .thread(&id)
        .ok_or_else(|| StoreError::message("thread missing"))?;
    assert_eq!(thread.lifecycle(), Lifecycle::ResolutionProposed);
    assert_eq!(thread.status(), Status::Open);
    assert!(thread.replies()[0].proposes_resolution());
    Ok(())
}

#[test]
fn replay_preserves_original_outcome_and_new_permission() -> Result<(), StoreError> {
    let file = TempFile::new("agent-replay-outcome")?;
    let author = keyed_author("copilot:one");
    let mut store = Store::open(&file.0)?;
    let id = store.annotate(
        Draft::on_file(Author::User, Path::new("a.md"), "question"),
        TEXT,
        1,
    )?;
    let command = AgentReplyCommand::new(author, 2, "fixed")
        .resolve()
        .idempotent("copilot:one", "reply-1");
    let first = store.agent_reply(&id, command.clone(), |_| Ok(TEXT.to_owned()))?;
    assert_eq!(first.into_value(), ResolutionOutcome::ResolutionProposed);
    store.set_auto_resolve(&id, AutoResolve::Enabled, 3)?;
    let before = fs::read_to_string(&file.0)
        .map_err(|error| StoreError::io(&file.0, error))?
        .lines()
        .count();
    let replay = store.agent_reply(&id, command, |_| {
        Err(StoreError::message("replay must not load source"))
    })?;
    assert!(replay.replayed());
    assert_eq!(replay.into_value(), ResolutionOutcome::ResolutionProposed);
    let thread = store
        .thread(&id)
        .ok_or_else(|| StoreError::message("thread missing"))?;
    assert_eq!(thread.auto_resolve(), AutoResolve::Enabled);
    assert_eq!(thread.replies().len(), 1);
    assert_eq!(
        fs::read_to_string(&file.0)
            .map_err(|error| StoreError::io(&file.0, error))?
            .lines()
            .count(),
        before
    );
    Ok(())
}

#[test]
fn direct_resolve_pins_head_in_one_event() -> Result<(), StoreError> {
    let file = TempFile::new("direct-resolve")?;
    let mut store = Store::open(&file.0)?;
    let id = store.annotate(
        Draft::on_file(Author::User, Path::new("a.md"), "question")
            .at_commit(Some("old".to_owned())),
        TEXT,
        1,
    )?;
    store.resolve(&id, Some("head"), 2)?;
    let raw = fs::read_to_string(&file.0).map_err(|error| StoreError::io(&file.0, error))?;
    assert_eq!(raw.lines().count(), 2);
    let last = raw
        .lines()
        .last()
        .ok_or_else(|| StoreError::message("missing event"))?;
    assert!(last.contains(r#""event":"resolve""#));
    assert!(last.contains(r#""commit":"head""#));
    assert_eq!(store.thread(&id).and_then(Thread::commit), Some("head"));
    Ok(())
}

#[test]
fn same_second_lifecycle_transitions_follow_event_order() -> Result<(), StoreError> {
    let file = TempFile::new("same-second")?;
    let mut store = Store::open(&file.0)?;
    let id = store.annotate(
        Draft::on_file(Author::User, Path::new("a.md"), "question"),
        TEXT,
        10,
    )?;
    store.agent_reply(
        &id,
        AgentReplyCommand::new(Author::agent("bot"), 10, "done").resolve(),
        |_| Ok(TEXT.to_owned()),
    )?;
    store.reply_user(&id, 10, "wait", UserSubmit::EnableAutoResolve)?;
    store.agent_reply(
        &id,
        AgentReplyCommand::new(Author::agent("bot"), 10, "now done")
            .resolve()
            .at_head(Some("head".to_owned())),
        |_| Ok(TEXT.to_owned()),
    )?;
    store.reopen(&id, 10)?;
    let thread = store
        .thread(&id)
        .ok_or_else(|| StoreError::message("thread missing"))?;
    assert_eq!(thread.lifecycle(), Lifecycle::Active);
    assert_eq!(thread.auto_resolve(), AutoResolve::Disabled);
    assert_eq!(thread.modified(), 10);
    Ok(())
}

#[test]
fn reanchor_time_survives_reply_resolve_and_reopen() -> Result<(), StoreError> {
    let file = TempFile::new("lasting-reanchor")?;
    let mut store = Store::open(&file.0)?;
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(3, 3),
            "question",
        ),
        TEXT,
        1,
    )?;
    store.relocate(
        &id,
        LineRange::new(3, 4),
        TEXT,
        PlacementContext::new(OriginVersion::working_tree(None)),
        2,
    )?;
    store.reply_user(&id, 3, "answer", UserSubmit::Normal)?;
    store.resolve(&id, Some("head"), 4)?;
    store.reopen(&id, 5)?;
    assert_eq!(store.thread(&id).and_then(Thread::reanchored_at), Some(2));
    assert_eq!(store.thread(&id).map(Thread::modified), Some(5));
    Ok(())
}

#[test]
fn activity_cursor_observes_consecutive_agent_messages_without_replay_duplicates()
-> Result<(), StoreError> {
    let file = TempFile::new("activity")?;
    let mut store = Store::open(&file.0)?;
    let id = store.annotate(
        Draft::on_file(Author::User, Path::new("a.md"), "question"),
        TEXT,
        1,
    )?;
    let cursor = store.activity_cursor();
    let first = AgentReplyCommand::new(keyed_author("copilot:one"), 2, "first")
        .idempotent("copilot:one", "reply-1");
    store.agent_reply(&id, first.clone(), |_| Ok(TEXT.to_owned()))?;
    store.agent_reply(
        &id,
        AgentReplyCommand::new(Author::agent("bot"), 3, "second"),
        |_| Ok(TEXT.to_owned()),
    )?;
    let before_replay = store.activity_cursor();
    let replay = store.agent_reply(&id, first, |_| {
        Err(StoreError::message("replay must not load source"))
    })?;
    assert!(replay.replayed());
    assert_eq!(store.activity_cursor(), before_replay);
    let activity = store.agent_activity_since(cursor).collect::<Vec<_>>();
    assert_eq!(activity.len(), 2);
    assert_eq!(activity[0].target(), MessageTarget::Reply(0));
    assert_eq!(activity[0].body(), "first");
    assert_eq!(activity[0].path(), Path::new("a.md"));
    assert_eq!(activity[0].range(), None);
    assert_eq!(activity[1].target(), MessageTarget::Reply(1));
    assert_eq!(activity[1].body(), "second");

    store.delete(&id, 4)?;
    let reloaded = Store::open(&file.0)?;
    assert_eq!(reloaded.agent_activity_since(cursor).count(), 2);
    Ok(())
}

#[test]
fn activity_cursor_observes_agent_opening_comments() -> Result<(), StoreError> {
    let file = TempFile::new("activity-opening")?;
    let mut store = Store::open(&file.0)?;
    let cursor = store.activity_cursor();
    let id = store.annotate(
        Draft::on_file(Author::agent("bot"), Path::new("a.md"), "finding"),
        TEXT,
        1,
    )?;
    let activity = store.agent_activity_since(cursor).collect::<Vec<_>>();
    assert_eq!(activity.len(), 1);
    assert_eq!(activity[0].thread(), &id);
    assert_eq!(activity[0].target(), MessageTarget::Comment);
    assert_eq!(activity[0].body(), "finding");
    assert_eq!(activity[0].path(), Path::new("a.md"));
    assert_eq!(activity[0].range(), None);
    Ok(())
}

#[test]
fn a_user_write_reloads_unseen_agent_activity() -> Result<(), StoreError> {
    let file = TempFile::new("activity-import")?;
    let mut observer = Store::open(&file.0)?;
    let id = observer.annotate(
        Draft::on_file(Author::User, Path::new("a.md"), "question"),
        TEXT,
        1,
    )?;
    let cursor = observer.activity_cursor();
    let mut writer = Store::open(&file.0)?;
    writer.agent_reply(
        &id,
        AgentReplyCommand::new(Author::agent("bot"), 2, "answer"),
        |_| Ok(TEXT.to_owned()),
    )?;

    observer.reply_user(&id, 3, "thanks", UserSubmit::Normal)?;
    let activity = observer.agent_activity_since(cursor).collect::<Vec<_>>();
    assert_eq!(activity.len(), 1);
    assert_eq!(activity[0].body(), "answer");
    Ok(())
}

#[test]
fn concurrent_agent_reply_retries_share_one_outcome() -> Result<(), StoreError> {
    let file = TempFile::new("agent-concurrent")?;
    let mut store = Store::open(&file.0)?;
    let id = store.annotate(
        Draft::on_file(Author::User, Path::new("a.md"), "question"),
        TEXT,
        1,
    )?;
    store.set_auto_resolve(&id, AutoResolve::Enabled, 2)?;
    let command = AgentReplyCommand::new(keyed_author("copilot:one"), 3, "done")
        .resolve()
        .at_head(Some("head".to_owned()))
        .idempotent("copilot:one", "reply-1");
    let first_path = file.0.clone();
    let second_path = file.0.clone();
    let first_id = id.clone();
    let second_id = id.clone();
    let first_command = command.clone();
    let first = std::thread::spawn(move || -> Result<_, StoreError> {
        Store::open(first_path)?.agent_reply(&first_id, first_command, |_| Ok(TEXT.to_owned()))
    });
    let second = std::thread::spawn(move || -> Result<_, StoreError> {
        Store::open(second_path)?.agent_reply(&second_id, command, |_| Ok(TEXT.to_owned()))
    });
    let first = first
        .join()
        .map_err(|_panic| StoreError::message("thread panicked"))??;
    let second = second
        .join()
        .map_err(|_panic| StoreError::message("thread panicked"))??;
    assert_eq!(*first.value(), ResolutionOutcome::Resolved);
    assert_eq!(*second.value(), ResolutionOutcome::Resolved);
    assert_ne!(first.replayed(), second.replayed());
    let reloaded = Store::open(&file.0)?;
    let thread = reloaded
        .thread(&id)
        .ok_or_else(|| StoreError::message("thread missing"))?;
    assert_eq!(thread.replies().len(), 1);
    assert_eq!(thread.lifecycle(), Lifecycle::Resolved);
    assert_eq!(thread.commit(), Some("head"));
    Ok(())
}

#[test]
fn user_submissions_enable_auto_resolve_atomically() -> Result<(), StoreError> {
    let file = TempFile::new("user-submit")?;
    let mut store = Store::open(&file.0)?;
    let id = store.annotate_user(
        Draft::on_file(Author::User, Path::new("a.md"), "question"),
        TEXT,
        1,
        UserSubmit::EnableAutoResolve,
    )?;
    assert_eq!(
        store.thread(&id).map(Thread::auto_resolve),
        Some(AutoResolve::Enabled)
    );
    store.agent_reply(
        &id,
        AgentReplyCommand::new(Author::agent("bot"), 2, "progress"),
        |_| Ok(TEXT.to_owned()),
    )?;
    store.edit_user(
        &id,
        MessageTarget::Comment,
        "clearer",
        3,
        UserSubmit::EnableAutoResolve,
    )?;
    assert_eq!(
        store.thread(&id).map(Thread::auto_resolve),
        Some(AutoResolve::Enabled)
    );
    assert_eq!(store.toggle_auto_resolve(&id, 4)?, AutoResolve::Disabled);
    store.resolve(&id, None, 5)?;
    let before = fs::read_to_string(&file.0)
        .map_err(|error| StoreError::io(&file.0, error))?
        .lines()
        .count();
    store.reply_user(&id, 6, "reopen", UserSubmit::EnableAutoResolve)?;
    assert_eq!(
        fs::read_to_string(&file.0)
            .map_err(|error| StoreError::io(&file.0, error))?
            .lines()
            .count(),
        before + 1
    );
    let thread = store
        .thread(&id)
        .ok_or_else(|| StoreError::message("thread missing"))?;
    assert_eq!(thread.lifecycle(), Lifecycle::Active);
    assert_eq!(thread.auto_resolve(), AutoResolve::Enabled);
    assert_eq!(thread.reopened(), Some(6));
    store.resolve(&id, None, 7)?;
    assert!(
        store
            .set_auto_resolve(&id, AutoResolve::Enabled, 8)
            .is_err()
    );
    Ok(())
}
#[test]
fn guarded_user_writes_preserve_a_resolved_draft_until_confirmed() -> Result<(), StoreError> {
    let file = TempFile::new("guarded-user-write")?;
    let mut store = Store::open(&file.0)?;
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(3, 3),
            "question",
        ),
        TEXT,
        1,
    )?;
    store.resolve(&id, Some("abc123"), 2)?;

    assert_eq!(
        store.reply_user_if_unresolved(&id, 3, "preserved reply", UserSubmit::EnableAutoResolve,)?,
        UserWriteOutcome::ReopenRequired
    );
    let resolved = store
        .thread(&id)
        .ok_or_else(|| StoreError::message("thread"))?;
    assert_eq!(resolved.lifecycle(), Lifecycle::Resolved);
    assert!(resolved.replies().is_empty());

    store.reply_user(&id, 4, "preserved reply", UserSubmit::EnableAutoResolve)?;
    let reopened = store
        .thread(&id)
        .ok_or_else(|| StoreError::message("thread"))?;
    assert_eq!(reopened.lifecycle(), Lifecycle::Active);
    assert_eq!(reopened.auto_resolve(), AutoResolve::Enabled);
    assert_eq!(reopened.replies().len(), 1);
    Ok(())
}

#[test]
fn user_writes_reject_a_thread_deleted_while_its_draft_was_open() -> Result<(), StoreError> {
    let file = TempFile::new("guarded-user-write-deleted")?;
    let mut creator = Store::open(&file.0)?;
    let reply_id = creator.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(3, 3),
            "reply target",
        ),
        TEXT,
        1,
    )?;
    let edit_id = creator.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(4, 4),
            "edit target",
        ),
        TEXT,
        2,
    )?;
    let mut stale = Store::open(&file.0)?;
    let mut deleter = Store::open(&file.0)?;
    deleter.delete(&reply_id, 3)?;
    deleter.delete(&edit_id, 4)?;
    let lines_after_delete = fs::read_to_string(&file.0)
        .map_err(|error| StoreError::io(&file.0, error))?
        .lines()
        .count();

    let reply =
        stale.reply_user_if_unresolved(&reply_id, 5, "must remain a draft", UserSubmit::Normal);
    assert!(
        reply
            .as_ref()
            .is_err_and(|error| error.to_string().contains("unknown thread"))
    );
    let edit = stale.edit_user_if_unresolved(
        &edit_id,
        MessageTarget::Comment,
        "must also remain a draft",
        6,
        UserSubmit::Normal,
    );
    assert!(
        edit.as_ref()
            .is_err_and(|error| error.to_string().contains("unknown thread"))
    );
    assert!(
        stale
            .reply_user(&reply_id, 7, "confirmed reopen", UserSubmit::Normal)
            .as_ref()
            .is_err_and(|error| error.to_string().contains("unknown thread")),
        "confirmation after deletion must not report success"
    );
    assert!(
        stale
            .edit_user(
                &edit_id,
                MessageTarget::Comment,
                "confirmed edit",
                8,
                UserSubmit::Normal,
            )
            .as_ref()
            .is_err_and(|error| error.to_string().contains("unknown thread")),
        "confirmed edit after deletion must not report success"
    );
    assert_eq!(
        fs::read_to_string(&file.0)
            .map_err(|error| StoreError::io(&file.0, error))?
            .lines()
            .count(),
        lines_after_delete,
        "failed writes append no ignored events"
    );
    Ok(())
}

#[test]
fn origin_survives_relocation_move_resolution_and_reopen() -> Result<(), StoreError> {
    let file = TempFile::new("immutable-origin")?;
    let mut store = Store::open(&file.0)?;
    let checkout = CheckoutIdentity::from_canonical_paths(
        file.0.clone(),
        file.0.parent().unwrap_or(Path::new("/")).to_path_buf(),
    );
    let provenance = Provenance::new(OriginVersion::commit("base"), OriginSide::Base)
        .with_comparison(ComparisonFacts::at_checkout(
            OriginVersion::commit("base"),
            OriginVersion::working_tree(Some("base".to_owned())),
            checkout.clone(),
        ))
        .with_working_tree(WorkingTreeFacts::new(
            Some("base".to_owned()),
            WorkingTreeState::Modified,
            None,
            checkout,
            FullFileDigest::from_bytes(TEXT.as_bytes()),
        ));
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("old.md"),
            LineRange::new(3, 3),
            "question",
        )
        .at_commit(Some("base".to_owned()))
        .with_provenance(provenance),
        TEXT,
        1,
    )?;
    let original = store
        .thread(&id)
        .ok_or_else(|| StoreError::message("missing origin thread"))?
        .origin()
        .clone();

    store.relocate(
        &id,
        LineRange::new(3, 4),
        TEXT,
        PlacementContext::new(OriginVersion::working_tree(None)),
        2,
    )?;
    store.resolve(&id, Some("resolved"), 5)?;
    store.reopen(&id, 6)?;

    let thread = store
        .thread(&id)
        .ok_or_else(|| StoreError::message("missing relocated thread"))?;
    assert_eq!(thread.origin(), &original);
    assert_eq!(thread.origin().path(), Path::new("old.md"));
    assert_eq!(thread.origin().range(), Some(LineRange::new(3, 3)));
    assert_eq!(thread.origin().version(), &OriginVersion::commit("base"));
    assert_eq!(thread.path(), Path::new("old.md"));
    assert_eq!(thread.range(), Some(LineRange::new(3, 4)));
    assert_eq!(thread.commit(), Some("resolved"));
    assert_eq!(thread.lifecycle(), Lifecycle::Active);
    Ok(())
}

#[test]
fn resolution_history_is_immutable_and_recent_order_ignores_metadata_edits()
-> Result<(), StoreError> {
    let file = TempFile::new("resolution-history")?;
    let mut store = Store::open(&file.0)?;
    let first = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(3, 3),
            "first",
        ),
        TEXT,
        1,
    )?;
    let second = store.annotate(
        Draft::on_file(Author::User, Path::new("b.md"), "second"),
        TEXT,
        2,
    )?;
    store.resolve(&first, Some("head-a"), 10)?;
    store.resolve_with_context(
        &second,
        &super::ResolutionContext::new(Author::User, Some("head-b".to_owned()))
            .at_checkout("checkout-b"),
        20,
    )?;
    store.relocate(
        &first,
        LineRange::new(3, 3),
        TEXT,
        PlacementContext::new(OriginVersion::working_tree(Some("head-a".to_owned()))),
        100,
    )?;

    let recent = store.recently_resolved();
    assert_eq!(
        recent.iter().map(|thread| thread.id()).collect::<Vec<_>>(),
        [&second, &first]
    );
    assert_eq!(
        recent[0]
            .latest_resolution()
            .map(super::ResolutionRecord::created),
        Some(20)
    );
    assert_eq!(
        recent[0]
            .latest_resolution()
            .and_then(|event| event.checkout()),
        Some("checkout-b")
    );
    assert_eq!(
        store
            .thread(&first)
            .map(|thread| thread.resolution_history().len()),
        Some(1)
    );

    store.reopen(&second, 30)?;
    assert!(
        store
            .recently_resolved()
            .iter()
            .all(|thread| thread.id() != &second)
    );
    store.resolve(&second, Some("head-c"), 40)?;
    assert_eq!(
        store.recently_resolved().first().map(|thread| thread.id()),
        Some(&second)
    );
    assert_eq!(
        store
            .thread(&second)
            .map(|thread| thread.resolution_history().len()),
        Some(2)
    );
    Ok(())
}

#[test]
fn archive_restore_preserves_lifecycle_history_and_disables_auto_resolve() -> Result<(), StoreError>
{
    let file = TempFile::new("archive-restore")?;
    let mut store = Store::open(&file.0)?;
    let id = store.annotate_user(
        Draft::on_file(Author::User, Path::new("a.md"), "question"),
        TEXT,
        1,
        UserSubmit::EnableAutoResolve,
    )?;
    store.resolve(&id, Some("head"), 2)?;
    let origin = store
        .thread(&id)
        .ok_or_else(|| StoreError::message("thread"))?
        .origin()
        .clone();
    store.archive_with_context(
        &id,
        ArchiveContext::new(Author::User, 3).at_checkout("main"),
    )?;
    assert!(store.threads().is_empty());
    let archived = store
        .thread(&id)
        .ok_or_else(|| StoreError::message("archived thread"))?;
    assert!(archived.is_archived());
    assert_eq!(archived.lifecycle(), Lifecycle::Resolved);
    assert_eq!(archived.auto_resolve(), AutoResolve::Disabled);
    assert_eq!(archived.origin(), &origin);

    store.restore(&id, 4)?;
    let restored = store
        .thread(&id)
        .ok_or_else(|| StoreError::message("restored thread"))?;
    assert!(!restored.is_archived());
    assert_eq!(restored.lifecycle(), Lifecycle::Resolved);
    assert_eq!(restored.resolution_history().len(), 1);
    assert_eq!(restored.archive_history().len(), 1);
    assert_eq!(restored.restore_history().len(), 1);
    assert_eq!(restored.auto_resolve(), AutoResolve::Disabled);
    let reloaded = Store::open(&file.0)?;
    assert_eq!(reloaded.threads().len(), 1);
    assert_eq!(reloaded.archived_threads().len(), 0);
    assert_eq!(
        reloaded
            .thread(&id)
            .map(|thread| thread.resolution_history().len()),
        Some(1)
    );
    Ok(())
}

#[test]
fn archive_and_restore_keep_creation_order_in_both_collections() -> Result<(), StoreError> {
    let file = TempFile::new("archive-order")?;
    let mut store = Store::open(&file.0)?;
    let first = store.annotate(
        Draft::on_file(Author::User, Path::new("first.md"), "first"),
        TEXT,
        1,
    )?;
    let second = store.annotate(
        Draft::on_file(Author::User, Path::new("second.md"), "second"),
        TEXT,
        2,
    )?;
    let third = store.annotate(
        Draft::on_file(Author::User, Path::new("third.md"), "third"),
        TEXT,
        3,
    )?;
    for id in [&first, &second, &third] {
        store.resolve(id, None, 10)?;
    }

    assert_eq!(
        store.archive_resolved(11)?,
        vec![first.clone(), second.clone(), third.clone()]
    );
    assert_eq!(
        store
            .archived_threads()
            .iter()
            .map(Thread::id)
            .collect::<Vec<_>>(),
        [&first, &second, &third]
    );
    assert!(store.threads().is_empty());

    store.restore(&third, 12)?;
    store.restore(&first, 13)?;
    store.restore(&second, 14)?;
    assert_eq!(
        store.threads().iter().map(Thread::id).collect::<Vec<_>>(),
        [&first, &second, &third]
    );
    assert!(store.archived_threads().is_empty());
    let reloaded = Store::open(&file.0)?;
    assert_eq!(
        reloaded
            .threads()
            .iter()
            .map(Thread::id)
            .collect::<Vec<_>>(),
        [&first, &second, &third]
    );
    assert!(reloaded.archived_threads().is_empty());
    Ok(())
}

#[test]
fn archive_resolved_rechecks_reopen_under_the_write_lock() -> Result<(), StoreError> {
    let file = TempFile::new("archive-race")?;
    let mut writer = Store::open(&file.0)?;
    let id = writer.annotate(
        Draft::on_file(Author::User, Path::new("a.md"), "question"),
        TEXT,
        1,
    )?;
    writer.resolve(&id, Some("head"), 2)?;
    let mut stale_archiver = Store::open(&file.0)?;
    let mut reopener = Store::open(&file.0)?;
    reopener.reopen(&id, 3)?;
    assert!(stale_archiver.archive_resolved(4)?.is_empty());
    assert!(
        !Store::open(&file.0)?
            .thread(&id)
            .is_some_and(Thread::is_archived)
    );
    Ok(())
}

#[test]
fn clear_board_archives_only_the_acknowledged_unchanged_slate() -> Result<(), StoreError> {
    let file = TempFile::new("clear-board")?;
    let mut store = Store::open(&file.0)?;
    let original = store.annotate(
        Draft::on_file(Author::User, Path::new("a.md"), "original"),
        TEXT,
        1,
    )?;
    let slate = store.board_slate();
    let new_thread = store.annotate(
        Draft::on_file(Author::User, Path::new("b.md"), "new"),
        TEXT,
        2,
    )?;
    let archived = store.clear_board(&slate, 3)?;
    assert_eq!(archived, vec![original.clone()]);
    assert!(store.thread(&original).is_some_and(Thread::is_archived));
    assert!(!store.thread(&new_thread).is_some_and(Thread::is_archived));

    let changed = store.annotate(
        Draft::on_file(Author::User, Path::new("c.md"), "changed"),
        TEXT,
        4,
    )?;
    let changed_slate = store.board_slate();
    store.reply_user(&changed, 5, "metadata", UserSubmit::Normal)?;
    assert!(
        store.clear_board(&changed_slate, 6).is_err(),
        "changed acknowledged state must reject the clear"
    );
    assert!(!store.thread(&changed).is_some_and(Thread::is_archived));
    Ok(())
}

#[test]
fn archived_fresh_reply_fails_but_keyed_replay_keeps_its_original_outcome() -> Result<(), StoreError>
{
    let file = TempFile::new("archive-replay")?;
    let author = Author::Agent {
        name: "bot".to_owned(),
        client: Some("mcp".to_owned()),
        id: Some("caller-1".to_owned()),
    };
    let mut store = Store::open(&file.0)?;
    let id = store.annotate(
        Draft::on_file(Author::User, Path::new("a.md"), "question"),
        TEXT,
        1,
    )?;
    let command =
        AgentReplyCommand::new(author.clone(), 2, "answer").idempotent("caller-1", "reply-1");
    let first = store.agent_reply(&id, command.clone(), |_| Ok(TEXT.to_owned()))?;
    assert_eq!(first.value(), &ResolutionOutcome::NotRequested);
    store.archive(&id, 3)?;

    let replay = store.agent_reply(&id, command, |_| {
        Err(StoreError::message(
            "replay must precede archive validation",
        ))
    })?;
    assert!(replay.replayed());
    assert_eq!(replay.value(), &ResolutionOutcome::NotRequested);
    let fresh = store.agent_reply(
        &id,
        AgentReplyCommand::new(author, 4, "fresh").idempotent("caller-1", "reply-2"),
        |_| Ok(TEXT.to_owned()),
    );
    assert!(
        fresh
            .as_ref()
            .is_err_and(|error| error.to_string().contains("archived"))
    );
    assert_eq!(
        store.thread(&id).map(|thread| thread.replies().len()),
        Some(1)
    );
    Ok(())
}
