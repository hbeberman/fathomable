//! Behaviour of theme parsing, inheritance, and lookup (ADR 0011).

use std::collections::HashMap;
use std::error::Error;
use std::io;

use fathomable_core::theme::{AnsiColor, BUILTIN_NAMES, Color, Key, Modifiers, Theme, ThemeError};

type TestResult = Result<(), Box<dyn Error>>;

fn from_map(files: &[(&str, &str)]) -> impl Fn(&str) -> io::Result<Option<String>> {
    let map: HashMap<String, String> = files
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    move |name| Ok(map.get(name).cloned())
}

fn must_fail(name: &str, files: &[(&str, &str)]) -> Result<ThemeError, Box<dyn Error>> {
    Ok(Theme::resolve(name, from_map(files))
        .err()
        .ok_or("expected the theme to be rejected")?)
}

#[test]
fn builtins_resolve_and_set_every_key() -> TestResult {
    for name in BUILTIN_NAMES {
        let theme = Theme::resolve(name, |_| Ok(None))?;
        assert_eq!(theme.name(), name);
        assert!(!theme.syntect().is_empty(), "{name} sets code.syntect");
        // A theme file may legitimately leave a key unset, but the built-ins
        // are expected to be complete so user themes have a full parent.
        for key in Key::all() {
            let style = theme.style(key);
            assert!(
                style.fg().is_some()
                    || style.bg().is_some()
                    || style.modifiers() != Modifiers::default(),
                "{name} leaves {} unset",
                key.as_str()
            );
        }
    }
    Ok(())
}

#[test]
fn list_focus_roles_are_distinct_from_chrome_and_inherit_together() -> TestResult {
    for name in BUILTIN_NAMES {
        let theme = Theme::resolve(name, |_| Ok(None))?;
        let backgrounds = [
            Key::UiListActive,
            Key::UiListInactive,
            Key::UiListHover,
            Key::UiHeader,
        ]
        .map(|key| theme.style(key).bg());
        assert!(backgrounds.iter().all(Option::is_some));
        for (index, bg) in backgrounds.iter().enumerate() {
            assert!(
                !backgrounds[index + 1..].contains(bg),
                "{name}: focus is not chrome or hover"
            );
        }
        assert!(theme.style(Key::UiListCursor).fg().is_some());
        let child = Theme::resolve(
            "child",
            from_map(&[("child", &format!("inherits \"{name}\""))]),
        )?;
        for key in [
            Key::UiListActive,
            Key::UiListInactive,
            Key::UiListCursor,
            Key::UiListHover,
        ] {
            assert_eq!(child.style(key), theme.style(key));
        }
    }
    Ok(())
}

#[test]
fn unknown_name_is_not_found() -> TestResult {
    let error = must_fail("nope", &[])?;
    assert!(error.is_not_found());
    assert!(error.to_string().contains("nope"));
    Ok(())
}

#[test]
fn user_file_shadows_builtin_and_inherits_default_dark_implicitly() -> TestResult {
    let source = from_map(&[("default-light", r##"colors { "ui.text" fg="#123456" }"##)]);
    let theme = Theme::resolve("default-light", source)?;
    assert_eq!(
        theme.style(Key::UiText).fg(),
        Some(Color::Rgb(0x12, 0x34, 0x56))
    );
    // Unset keys come from the built-in default-dark, not from built-in light.
    let dark = Theme::resolve("default-dark", |_| Ok(None))?;
    assert_eq!(theme.style(Key::UiLinenr), dark.style(Key::UiLinenr));
    assert_eq!(theme.syntect(), dark.syntect());
    Ok(())
}

#[test]
fn inherits_chain_overrides_parent_and_reuses_its_palette() -> TestResult {
    let source = from_map(&[
        (
            "base",
            r##"palette { accent "#ff0000" }
colors { "ui.linenr" fg="accent"; "ui.selection" bg="accent" }"##,
        ),
        (
            "child",
            r##"inherits "base"
palette { accent "blue" }
colors { "ui.linenr" fg="#00ff00" }"##,
        ),
    ]);
    let theme = Theme::resolve("child", source)?;
    assert_eq!(
        theme.style(Key::UiLinenr).fg(),
        Some(Color::Rgb(0, 0xff, 0))
    );
    // The parent's key resolved against the parent's palette, not the child's.
    assert_eq!(
        theme.style(Key::UiSelection).bg(),
        Some(Color::Rgb(0xff, 0, 0))
    );
    Ok(())
}

#[test]
fn child_palette_names_reference_parent_palette() -> TestResult {
    let source = from_map(&[(
        "mine",
        r#"inherits "default-dark"
colors { "diff.minus" fg="red" }"#,
    )]);
    let theme = Theme::resolve("mine", source)?;
    // `red` here is the default-dark palette entry, which shadows ANSI red.
    assert_eq!(
        theme.style(Key::DiffMinus).fg(),
        Some(Color::Rgb(0xd5, 0x4e, 0x53))
    );
    Ok(())
}

#[test]
fn inherits_cycle_is_an_error() -> TestResult {
    let error = must_fail("a", &[("a", r#"inherits "b""#), ("b", r#"inherits "a""#)])?;
    assert!(error.to_string().contains("cycle"), "{error}");
    Ok(())
}

#[test]
fn heading_levels_fall_back_to_markup_heading() -> TestResult {
    let source = from_map(&[(
        "h",
        r##"colors { "markup.heading" fg="#aaaaaa" mods="bold"; "markup.heading.2" fg="#bbbbbb" }"##,
    )]);
    let theme = Theme::resolve("h", source)?;
    assert_eq!(
        theme.style(Key::MarkupHeadingLevel(1)).fg(),
        Some(Color::Rgb(0xaa, 0xaa, 0xaa))
    );
    assert_eq!(
        theme.style(Key::MarkupHeadingLevel(2)).fg(),
        Some(Color::Rgb(0xbb, 0xbb, 0xbb))
    );
    assert!(!theme.style(Key::MarkupHeadingLevel(2)).modifiers().bold);
    Ok(())
}

#[test]
fn positional_string_is_foreground_and_default_unsets() -> TestResult {
    let source = from_map(&[(
        "p",
        r#"colors { "diff.plus" "cyan"; "ui.text" fg="default" bg="default" }"#,
    )]);
    let theme = Theme::resolve("p", source)?;
    assert_eq!(
        theme.style(Key::DiffPlus).fg(),
        Some(Color::Ansi(AnsiColor::Cyan))
    );
    assert_eq!(theme.style(Key::UiText).fg(), None);
    assert_eq!(theme.style(Key::UiText).bg(), None);
    Ok(())
}

#[test]
fn modifiers_parse_as_space_separated_words() -> TestResult {
    let source = from_map(&[(
        "m",
        r#"colors { "markup.link" mods="underline italic strikethrough" }"#,
    )]);
    let mods = Theme::resolve("m", source)?
        .style(Key::MarkupLink)
        .modifiers();
    assert!(mods.underline && mods.italic && mods.strikethrough);
    assert!(!mods.bold);
    Ok(())
}

#[test]
fn errors_carry_a_location() -> TestResult {
    let cases: &[(&str, &str, usize)] = &[
        (
            "colors {\n  \"ui.nope\" fg=\"red\"\n}",
            "unknown theme key `ui.nope`",
            2,
        ),
        (
            "colors {\n  \"ui.text\" fg=\"#12345\"\n}",
            "unknown colour `#12345`",
            2,
        ),
        (
            "colors {\n  \"ui.text\" fg=\"notacolour\"\n}",
            "unknown colour `notacolour`",
            2,
        ),
        (
            "colors {\n  \"ui.text\" mods=\"blink\"\n}",
            "unknown modifier `blink`",
            2,
        ),
        (
            "colors {\n  \"ui.text\" colour=\"red\"\n}",
            "unknown property `colour`",
            2,
        ),
        ("\n\nglyphs {\n}", "unknown node `glyphs`", 3),
        ("code {\n  tmtheme \"x\"\n}", "unknown node `tmtheme`", 2),
        ("\ninherits 3", "expected a string", 2),
        ("colors {\n  \"ui.text\" fg=1\n}", "expected a string", 2),
        ("colors {\n  \"ui.text\" {\n  }\n}", "expected", 2),
    ];
    for (text, expected, line) in cases {
        let error = must_fail("bad", &[("bad", text)])?;
        let message = error.to_string();
        assert!(message.contains(expected), "{text:?} gave {message}");
        let location = error
            .location()
            .ok_or_else(|| format!("{text:?} has no location"))?;
        assert_eq!(location.line(), *line, "{text:?}: {location}");
        assert_eq!(error.theme(), "bad");
    }
    Ok(())
}

#[test]
fn syntax_errors_carry_a_location_and_message() -> TestResult {
    let error = must_fail("bad", &[("bad", "colors {\n  \"ui.text\" fg=\n}")])?;
    assert!(error.location().is_some(), "{error}");
    assert!(error.to_string().contains("at "), "{error}");
    Ok(())
}

#[test]
fn error_in_parent_names_the_parent() -> TestResult {
    let error = must_fail(
        "child",
        &[("child", r#"inherits "parent""#), ("parent", "bogus {}")],
    )?;
    assert_eq!(error.theme(), "parent");
    Ok(())
}

#[test]
fn io_errors_propagate() -> TestResult {
    let error = Theme::resolve("x", |_| Err(io::Error::other("disk on fire")))
        .err()
        .ok_or("expected an error")?;
    assert!(!error.is_not_found());
    assert!(error.to_string().contains("disk on fire"));
    Ok(())
}

#[test]
fn key_names_round_trip() -> TestResult {
    for key in Key::all() {
        let name = key.as_str();
        assert!(!name.is_empty());
        let text = format!("colors {{ \"{name}\" fg=\"#010203\" }}");
        let theme = Theme::resolve("k", from_map(&[("k", &text)]))?;
        assert_eq!(theme.style(key).fg(), Some(Color::Rgb(1, 2, 3)), "{name}");
    }
    Ok(())
}
