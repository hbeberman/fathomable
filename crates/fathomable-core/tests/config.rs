//! Behaviour of `config.kdl` reading (ADR 0008).

use std::collections::BTreeSet;
use std::error::Error;

use fathomable_core::XdgDirs;
use fathomable_core::config::{Config, ConfigError, DiffMode};
use fathomable_core::theme::DEFAULT_THEME;

type TestResult = Result<(), Box<dyn Error>>;

fn must_fail(text: &str) -> Result<ConfigError, Box<dyn Error>> {
    Ok(Config::parse(text)
        .err()
        .ok_or_else(|| format!("expected {text:?} to be rejected"))?)
}

#[test]
fn empty_config_is_default() -> TestResult {
    assert_eq!(Config::parse("")?, Config::default());
    assert_eq!(Config::default().theme(), DEFAULT_THEME);
    Ok(())
}

#[test]
fn default_toast_duration_is_five_seconds() {
    assert_eq!(
        Config::default().watch().toast,
        std::time::Duration::from_secs(5)
    );
}

#[test]
fn theme_is_read() -> TestResult {
    assert_eq!(
        Config::parse("theme \"default-light\"")?.theme(),
        "default-light"
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
    assert_eq!(Config::load(&dirs, Some(&path))?.theme(), "mine");

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

/// The `(block, key)` pairs a KDL config text sets, comments stripped.
fn keys(text: &str) -> BTreeSet<(String, String)> {
    let mut block = String::new();
    let mut keys = BTreeSet::new();
    for line in text.lines() {
        let line = line.split("//").next().unwrap_or_default().trim();
        let Some(name) = line.split_whitespace().next() else {
            continue;
        };
        if name == "}" {
            block.clear();
        } else if line.ends_with('{') {
            name.clone_into(&mut block);
        } else if !name.starts_with('"') {
            keys.insert((block.clone(), name.to_owned()));
        }
    }
    keys
}

/// The text form of a config reads back as an equal value, for the
/// defaults and for a config with every setting changed.
#[test]
fn text_form_round_trips() -> TestResult {
    let default = Config::default();
    assert_eq!(Config::parse(&default.to_string())?, default);

    let full = Config::parse(
        r#"
theme "mine \"quoted\" \\ back"
watch { toast 0; ignore "target/**" "a b" "c\"d"; debounce 1 }
markdown { extensions "txt"; names "notes" }
viewer { max-file-size-mib 1 }
layout {
    menu-bar #false
    sidebar {
        visible #false
        files #true
        threads #false
        width 1
        split 2
    }

}
threads { stubs #false; stubs-resolved #true }
diff { mode "off"; context 0; ignore-whitespace #true }
user { name "O'Brien" }
"#,
    )?;
    assert_ne!(full, default);
    assert_eq!(full.diff().mode, DiffMode::Off);
    let text = full.to_string();
    assert_eq!(Config::parse(&text)?, full, "{text}");
    assert_eq!(keys(&text), keys(&default.to_string()));
    Ok(())
}

#[test]
fn obsolete_jump_block_is_rejected() -> TestResult {
    let error = must_fail("jump { toast 5000 }")?;
    assert!(
        error.to_string().contains("unknown setting `jump`"),
        "{error}"
    );
    Ok(())
}

#[test]
fn text_form_explains_every_setting() {
    let text = Config::default().to_string();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.ends_with('{') || line == "}" {
            continue;
        }
        assert!(
            line.split_once("//")
                .is_some_and(|(_, comment)| !comment.trim().is_empty()),
            "setting has no usage comment: {line}"
        );
    }
}

/// The guide's example matches the default text form, including comments,
/// and still parses and round-trips.
#[test]
fn guide_example_matches_the_text_form() -> TestResult {
    let guide = std::fs::read_to_string(fathomable_testing::repo_file("docs/guide.md"))?;
    let section = guide
        .split("## Configuration")
        .nth(1)
        .ok_or("guide configuration section not found")?;
    let example = section
        .split("```kdl\n")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .ok_or("guide config example not found")?;
    let config = Config::parse(example)?;
    assert_eq!(Config::parse(&config.to_string())?, config);
    assert_eq!(example, Config::default().to_string());
    Ok(())
}
