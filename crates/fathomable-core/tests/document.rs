use std::error::Error;
use std::fs;
use std::path::PathBuf;

use fathomable_core::Document;

type TestResult = Result<(), Box<dyn Error>>;

fn scratch_file(name: &str, contents: &[u8]) -> Result<PathBuf, Box<dyn Error>> {
    let dir = std::env::temp_dir().join(format!("fathomable-core-test-{}", std::process::id()));
    fs::create_dir_all(&dir)?;
    let path = dir.join(name);
    fs::write(&path, contents)?;
    Ok(path)
}

#[test]
fn load_reads_path_and_text() -> TestResult {
    let path = scratch_file("load.md", b"# Hello\n")?;
    let document = Document::load(&path)?;
    assert_eq!(document.path(), path);
    assert_eq!(document.text(), "# Hello\n");
    Ok(())
}

#[test]
fn reload_reports_whether_text_changed() -> TestResult {
    let path = scratch_file("reload.md", b"one\n")?;
    let mut document = Document::load(&path)?;
    assert!(!document.reload()?);
    fs::write(&path, "two\n")?;
    assert!(document.reload()?);
    assert_eq!(document.text(), "two\n");
    Ok(())
}

#[test]
fn reload_keeps_previous_text_on_error() -> TestResult {
    let path = scratch_file("vanishing.md", b"kept\n")?;
    let mut document = Document::load(&path)?;
    fs::remove_file(&path)?;
    let Err(error) = document.reload() else {
        return Err("reload of a removed file must fail".into());
    };
    assert_eq!(error.path(), path);
    assert!(error.source().is_some());
    assert_eq!(document.text(), "kept\n");
    Ok(())
}

#[test]
fn load_rejects_non_utf8() -> TestResult {
    let path = scratch_file("binary.bin", &[0xff, 0xfe])?;
    if Document::load(&path).is_ok() {
        return Err("non-UTF-8 input must be rejected".into());
    }
    Ok(())
}
