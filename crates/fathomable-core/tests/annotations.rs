//! Serialized line ranges preserve the invariants used by anchors and stores.

use std::error::Error;
use std::fs;
use std::path::Path;

use fathomable_core::annotations::{
    Anchor, Author, ContentIdentity, Draft, LineRange, OriginSide, OriginVersion, Provenance, Store,
};
use fathomable_core::workspace::CommitId;
use fathomable_testing::TempDir;
use serde_json::json;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn deserialized_line_ranges_are_safe_for_anchors() -> TestResult {
    let text = "first\nsecond\n";
    for (start, end, len) in [(1, 1, 1), (1, 2, 2), (2, 2, 1)] {
        let encoded = json!({"start": start, "end": end});
        let range: LineRange = serde_json::from_value(encoded.clone())?;
        assert_eq!((range.start(), range.end(), range.len()), (start, end, len));
        assert!(!range.is_empty());
        assert!(range.contains(start) && range.contains(end));
        let anchor = Anchor::capture(text, range).ok_or("valid range was not captured")?;
        assert_eq!(anchor.locate(text, range), Some(range));
        assert_eq!(serde_json::to_value(range)?, encoded);
    }
    for (start, end, len) in [(1, usize::MAX, usize::MAX), (usize::MAX, usize::MAX, 1)] {
        let range: LineRange = serde_json::from_value(json!({"start": start, "end": end}))?;
        assert_eq!(range.len(), len);
        assert_eq!(Anchor::capture(text, range), None);
    }
    Ok(())
}

#[test]
fn deserialization_rejects_zero_and_reversed_line_ranges() -> TestResult {
    for (start, end) in [(0, 0), (0, 1), (1, 0), (2, 1), (5, 2)] {
        let result = serde_json::from_value::<LineRange>(json!({"start": start, "end": end}));
        let error = result
            .err()
            .ok_or("invalid serialized range was accepted")?;
        assert!(
            error.to_string().contains("line range"),
            "{start}..={end}: {error}"
        );
    }
    Ok(())
}

#[test]
fn deserialization_still_requires_both_unsigned_bounds() {
    for input in [
        r#"{"start":1}"#,
        r#"{"end":1}"#,
        r#"{"start":-1,"end":1}"#,
        r#"{"start":1,"end":null}"#,
        r#"{"start":"1","end":1}"#,
        r#"{"start":1,"start":2,"end":2}"#,
    ] {
        assert!(serde_json::from_str::<LineRange>(input).is_err(), "{input}");
    }
}

#[test]
fn store_rejects_invalid_ranges_without_rewriting_the_log() -> TestResult {
    let dir = TempDir::new("serialized-line-range")?;
    let path = dir.0.join("threads.jsonl");
    Store::open(&path)?.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "comment",
        ),
        "first\nsecond\n",
        1,
    )?;
    let original = fs::read_to_string(&path)?;
    for (start, end) in [(0, 1), (2, 1)] {
        let mut event: serde_json::Value = serde_json::from_str(original.trim_end())?;
        event["range"] = json!({"start": start, "end": end});
        let corrupted = format!("{event}\n");
        fs::write(&path, &corrupted)?;
        let error = Store::open(&path)
            .err()
            .ok_or("store accepted an invalid range")?;
        let message = error.to_string();
        assert!(message.contains("threads.jsonl line 1"), "{message}");
        assert!(message.contains("line range"), "{message}");
        assert_eq!(fs::read_to_string(&path)?, corrupted);
    }
    fs::write(&path, original)?;
    assert_eq!(Store::open(&path)?.threads().len(), 1);
    Ok(())
}

fn keyed_author() -> Author {
    Author::Agent {
        name: "reviewer".to_owned(),
        client: Some("copilot-cli".to_owned()),
        id: Some("copilot:selected-source".to_owned()),
    }
}

fn selected_draft(commit: &CommitId) -> Draft {
    Draft::new(
        keyed_author(),
        Path::new("a.md"),
        LineRange::new(1, 1),
        "comment",
    )
    .with_provenance(
        Provenance::new(
            OriginVersion::commit(commit.as_str()),
            OriginSide::Unspecified,
        )
        .with_content(ContentIdentity::from_text("first\nsecond\n")),
    )
    .at_selected_commit(commit.clone())
}

#[test]
fn commit_source_draft_records_exact_immutable_origin() -> TestResult {
    let dir = TempDir::new("selected-commit-origin")?;
    let path = dir.0.join("threads.jsonl");
    let commit = CommitId::parse("0123456789abcdef0123456789abcdef01234567")?;
    let mut store = Store::open(&path)?;
    let id = store
        .annotate_idempotent(selected_draft(&commit), 1, "selected", |_| {
            Ok("first\nsecond\n".to_owned())
        })?
        .into_value();
    let thread = store.thread(&id).ok_or("selected thread is missing")?;

    assert_eq!(
        thread.origin_version(),
        &OriginVersion::commit(commit.as_str())
    );
    assert_eq!(thread.origin_side(), OriginSide::Unspecified);
    assert_eq!(thread.commit(), Some(commit.as_str()));
    assert_eq!(
        thread.origin().content().map(ContentIdentity::bytes),
        Some("first\nsecond\n".len())
    );
    Ok(())
}

#[test]
fn commit_source_key_replays_only_for_the_same_selection() -> TestResult {
    let dir = TempDir::new("selected-commit-replay")?;
    let path = dir.0.join("threads.jsonl");
    let commit_c = CommitId::parse("1111111111111111111111111111111111111111")?;
    let commit_d = CommitId::parse("2222222222222222222222222222222222222222")?;
    let mut store = Store::open(&path)?;
    let first = store.annotate_idempotent(selected_draft(&commit_c), 1, "same", |_| {
        Ok("first\nsecond\n".to_owned())
    })?;
    assert!(!first.replayed());
    drop(store);

    let mut store = Store::open(&path)?;
    let replay = store.annotate_idempotent(selected_draft(&commit_c), 2, "same", |_| {
        Err(fathomable_core::annotations::StoreError::message(
            "selected replay loaded source",
        ))
    })?;
    assert!(replay.replayed());
    assert_eq!(replay.value(), first.value());

    let different = store.annotate_idempotent(selected_draft(&commit_d), 3, "same", |_| {
        Ok("first\nsecond\n".to_owned())
    });
    assert!(
        different
            .as_ref()
            .is_err_and(|error| error.to_string().contains("conflicts")),
        "{different:?}"
    );
    let unselected = store.annotate_idempotent(
        Draft::new(
            keyed_author(),
            Path::new("a.md"),
            LineRange::new(1, 1),
            "comment",
        ),
        4,
        "same",
        |_| Ok("first\nsecond\n".to_owned()),
    );
    assert!(
        unselected
            .as_ref()
            .is_err_and(|error| error.to_string().contains("conflicts")),
        "{unselected:?}"
    );

    store.annotate_idempotent(
        Draft::new(
            keyed_author(),
            Path::new("a.md"),
            LineRange::new(1, 1),
            "comment",
        ),
        5,
        "legacy-first",
        |_| Ok("first\nsecond\n".to_owned()),
    )?;
    let reverse = store.annotate_idempotent(selected_draft(&commit_c), 6, "legacy-first", |_| {
        Ok("first\nsecond\n".to_owned())
    });
    assert!(
        reverse
            .as_ref()
            .is_err_and(|error| error.to_string().contains("conflicts")),
        "{reverse:?}"
    );
    assert_eq!(store.threads().len(), 2);
    Ok(())
}

#[test]
fn commit_source_replay_survives_resolution_reopen_and_archive() -> TestResult {
    let dir = TempDir::new("selected-commit-lifecycle-replay")?;
    let path = dir.0.join("threads.jsonl");
    let commit = CommitId::parse("1111111111111111111111111111111111111111")?;
    let mut store = Store::open(&path)?;
    let id = store
        .annotate_idempotent(selected_draft(&commit), 1, "same", |_| {
            Ok("first\nsecond\n".to_owned())
        })?
        .into_value();
    store.resolve(&id, Some("2222222222222222222222222222222222222222"), 2)?;

    for state in ["resolved", "reopened", "archived", "deleted"] {
        match state {
            "reopened" => store.reopen(&id, 3)?,
            "archived" => store.archive(&id, 4)?,
            "deleted" => {
                store.restore(&id, 5)?;
                store.delete(&id, 6)?;
            }
            _ => {}
        }
        let before = fs::read(&path)?;
        store = Store::open(&path)?;
        let probe = store.probe_start_idempotency(&selected_draft(&commit), "same");
        let replay = store.annotate_idempotent(selected_draft(&commit), 6, "same", |_| {
            Err(fathomable_core::annotations::StoreError::message(
                "replay must not load selected source",
            ))
        });
        if state == "deleted" {
            assert!(probe.is_err_and(|error| error.to_string().contains("was deleted")));
            assert!(replay.is_err_and(|error| error.to_string().contains("was deleted")));
        } else {
            assert_eq!(probe?, Some(id.clone()), "{state}");
            let replay = replay?;
            assert!(replay.replayed(), "{state}");
            assert_eq!(replay.value(), &id, "{state}");
        }
        assert_eq!(fs::read(&path)?, before, "{state}");
    }
    Ok(())
}

#[test]
fn commit_source_rejects_origin_disagreement_before_probe_or_write() -> TestResult {
    let dir = TempDir::new("selected-commit-mismatch")?;
    let path = dir.0.join("threads.jsonl");
    let commit_c = CommitId::parse("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")?;
    let commit_d = CommitId::parse("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")?;
    let draft = selected_draft(&commit_c)
        .at_source(OriginVersion::commit(commit_d.as_str()), OriginSide::Base);
    let store = Store::open(&path)?;

    let error = store
        .probe_start_idempotency(&draft, "mismatch")
        .err()
        .ok_or("mismatched selected source was accepted")?;
    assert!(error.to_string().contains("does not agree"), "{error}");
    assert!(!path.exists(), "a rejected probe must not create the store");
    Ok(())
}
