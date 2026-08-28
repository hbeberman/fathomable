//! Behaviour of `config.kdl` reading (ADR 0008).

use std::error::Error;

use fathomable_core::XdgDirs;
use fathomable_core::config::{Config, ConfigError};

type TestResult = Result<(), Box<dyn Error>>;

fn must_fail(text: &str) -> Result<ConfigError, Box<dyn Error>> {
    Ok(Config::parse(text)
        .err()
        .ok_or_else(|| format!("expected {text:?} to be rejected"))?)
}

#[test]
fn empty_config_is_default() -> TestResult {
    assert_eq!(Config::parse("")?, Config::default());
    assert_eq!(Config::default().theme(), None);
    Ok(())
}

#[test]
fn theme_is_read() -> TestResult {
    assert_eq!(
        Config::parse("theme \"default-light\"")?.theme(),
        Some("default-light")
    );
    Ok(())
}

#[test]
fn unknown_setting_is_an_error_with_a_line() -> TestResult {
    let error = must_fail("theme \"x\"\n\nfollwo #true")?;
    assert_eq!(error.line(), Some(3));
    assert!(
        error.to_string().contains("unknown setting `follwo`"),
        "{error}"
    );
    Ok(())
}

#[test]
fn theme_needs_one_string() -> TestResult {
    for text in ["theme", "theme 1", "theme \"a\" \"b\"", "theme name=\"a\""] {
        let error = must_fail(text)?;
        assert_eq!(error.line(), Some(1), "{text}");
    }
    Ok(())
}

#[test]
fn invalid_kdl_is_an_error() -> TestResult {
    must_fail("theme \"unterminated")?;
    Ok(())
}

#[test]
fn missing_file_is_default_and_explicit_path_is_read() -> TestResult {
    let dir = std::env::temp_dir().join(format!("fathomable-config-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let dirs = XdgDirs::resolve(|name| (name == "XDG_CONFIG_HOME").then(|| dir.clone().into()));
    assert_eq!(Config::load(&dirs, None)?, Config::default());

    let path = dir.join("custom.kdl");
    std::fs::write(&path, "theme \"mine\"")?;
    assert_eq!(Config::load(&dirs, Some(&path))?.theme(), Some("mine"));

    std::fs::write(&path, "nope 1")?;
    let error = Config::load(&dirs, Some(&path))
        .err()
        .ok_or("expected an error")?;
    assert!(
        error.to_string().starts_with(&path.display().to_string()),
        "{error}"
    );
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn viewer_block_sets_the_size_ceiling_in_mib() -> TestResult {
    use fathomable_core::content::MIB;

    assert_eq!(Config::default().viewer().max_file_size_mib, 64);
    let config = Config::parse("viewer {\n    max-file-size-mib 512\n}\n")?;
    assert_eq!(config.viewer().max_file_size_mib, 512);
    assert_eq!(config.viewer().max_file_bytes(), 512 * MIB);
    let error = must_fail("viewer {\n    max-file-size-mib -1\n}\n")?;
    assert_eq!(error.line(), Some(2));
    let error = must_fail("viewer {\n    max-bytes 1\n}\n")?;
    assert!(
        error
            .to_string()
            .contains("unknown viewer setting `max-bytes`"),
        "{error}"
    );
    Ok(())
}
