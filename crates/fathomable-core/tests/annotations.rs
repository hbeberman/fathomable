//! Serialized line ranges preserve the invariants used by anchors and stores.

use std::error::Error;
use std::fs;
use std::path::Path;

use fathomable_core::annotations::{Anchor, Author, Draft, LineRange, Store};
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
