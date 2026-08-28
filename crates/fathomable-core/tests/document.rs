use std::error::Error;
use std::fs;
use std::path::PathBuf;

use fathomable_core::Document;
use fathomable_core::content::{Attr, Content, Format, Policy};

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
    let document = Document::load(&path, Policy::default())?;
    assert_eq!(document.path(), path);
    assert_eq!(document.text(), Some("# Hello\n"));
    Ok(())
}

#[test]
fn reload_reports_whether_text_changed() -> TestResult {
    let path = scratch_file("reload.md", b"one\n")?;
    let mut document = Document::load(&path, Policy::default())?;
    assert!(!document.reload()?);
    fs::write(&path, "two\n")?;
    assert!(document.reload()?);
    assert_eq!(document.text(), Some("two\n"));
    Ok(())
}

#[test]
fn reload_keeps_previous_text_on_error() -> TestResult {
    let path = scratch_file("vanishing.md", b"kept\n")?;
    let mut document = Document::load(&path, Policy::default())?;
    fs::remove_file(&path)?;
    let Err(error) = document.reload() else {
        return Err("reload of a removed file must fail".into());
    };
    assert_eq!(error.path(), path);
    assert!(error.source().is_some());
    assert_eq!(document.text(), Some("kept\n"));
    Ok(())
}

#[test]
fn load_rejects_non_utf8_text() -> TestResult {
    // No NUL, so git would call it text; it still cannot be shown.
    let path = scratch_file("latin1.txt", &[0xff, 0xfe, b'\n'])?;
    if Document::load(&path, Policy::default()).is_ok() {
        return Err("non-UTF-8 text must be rejected".into());
    }
    Ok(())
}

#[test]
fn a_nul_makes_a_binary_and_the_magic_names_it() -> TestResult {
    let path = scratch_file("plugin.wasm", b"\0asm\x01\0\0\0rest")?;
    let document = Document::load(&path, Policy::default())?;
    assert_eq!(document.text(), None);
    assert_eq!(
        *document.content(),
        Content::Binary {
            size: 12,
            format: Some(Format::WebAssembly),
        }
    );
    Ok(())
}

#[test]
fn the_diff_attribute_overrides_the_bytes() -> TestResult {
    let forced_binary = scratch_file("notes.dat", b"plain text\n")?;
    let policy = Policy {
        attr: Attr::Binary,
        ..Policy::default()
    };
    assert!(
        Document::load(&forced_binary, policy)?
            .content()
            .is_binary()
    );

    let forced_text = scratch_file("nul.txt", b"a\0b\n")?;
    let policy = Policy {
        attr: Attr::Text,
        ..Policy::default()
    };
    assert_eq!(Document::load(&forced_text, policy)?.text(), Some("a\0b\n"));
    Ok(())
}

#[test]
fn text_over_the_ceiling_is_not_read_but_binaries_still_are_sniffed() -> TestResult {
    let big = scratch_file("big.log", &[b'x'; 100])?;
    let policy = Policy {
        max_bytes: 10,
        ..Policy::default()
    };
    assert_eq!(
        *Document::load(&big, policy)?.content(),
        Content::TooLarge {
            size: 100,
            max_bytes: 10,
        }
    );

    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.resize(100, 0);
    let image = scratch_file("big.png", &bytes)?;
    assert_eq!(
        *Document::load(&image, policy)?.content(),
        Content::Binary {
            size: 100,
            format: Some(Format::Png),
        }
    );

    let mut document = Document::load(&big, policy)?;
    fs::write(&big, "small\n")?;
    assert!(document.reload()?);
    assert_eq!(document.text(), Some("small\n"));
    Ok(())
}
