// @okf-doc: /decisions/0011-theme-schema.md
//! KDL theme files: parsing, palette resolution, inheritance, and lookup.
//!
//! A theme maps the [`Key`] vocabulary from ADR 0011 to a [`Style`] made of
//! optional foreground and background [`Color`]s and [`Modifiers`]. Themes
//! are resolved by name: a file in the user's themes directory shadows a
//! built-in of the same name, and `inherits` chains end at the built-in
//! `default-dark`, so every key always has a value.
//!
//! # Examples
//!
//! ```
//! use fathomable_core::theme::{Key, Theme};
//!
//! let theme = Theme::resolve("default-dark", |_| Ok(None)).unwrap();
//! assert!(theme.style(Key::MarkupHeading).modifiers().bold);
//! assert_eq!(theme.syntect(), "base16-ocean.dark");
//! ```

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

use kdl::{KdlDocument, KdlEntry, KdlNode};

use crate::XdgDirs;

/// Name of the theme used when neither config nor `--theme` chooses one.
pub const DEFAULT_THEME: &str = "default-dark";

/// Names of the themes compiled into this crate.
pub const BUILTIN_NAMES: [&str; 2] = ["default-dark", "default-light"];

const BUILTINS: [(&str, &str); 2] = [
    ("default-dark", include_str!("../themes/default-dark.kdl")),
    ("default-light", include_str!("../themes/default-light.kdl")),
];

/// One of the sixteen terminal palette colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(missing_docs, reason = "variants are the ANSI colour names")]
pub enum AnsiColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
}

impl AnsiColor {
    const NAMES: [(&'static str, Self); 16] = [
        ("black", Self::Black),
        ("red", Self::Red),
        ("green", Self::Green),
        ("yellow", Self::Yellow),
        ("blue", Self::Blue),
        ("magenta", Self::Magenta),
        ("cyan", Self::Cyan),
        ("white", Self::White),
        ("bright-black", Self::BrightBlack),
        ("bright-red", Self::BrightRed),
        ("bright-green", Self::BrightGreen),
        ("bright-yellow", Self::BrightYellow),
        ("bright-blue", Self::BrightBlue),
        ("bright-magenta", Self::BrightMagenta),
        ("bright-cyan", Self::BrightCyan),
        ("bright-white", Self::BrightWhite),
    ];

    fn from_name(name: &str) -> Option<Self> {
        Self::NAMES
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, color)| *color)
    }
}

/// A concrete colour: true colour or one of the terminal's own sixteen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Color {
    /// A 24-bit colour written as `#rrggbb`.
    Rgb(u8, u8, u8),
    /// A named terminal colour, drawn from the user's terminal palette.
    Ansi(AnsiColor),
}

/// Text attributes a style may set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each modifier is an independent on/off attribute a theme may set"
)]
pub struct Modifiers {
    /// Bold weight.
    pub bold: bool,
    /// Dimmed intensity.
    pub dim: bool,
    /// Italic slant.
    pub italic: bool,
    /// Underlined text.
    pub underline: bool,
    /// Swapped foreground and background.
    pub reversed: bool,
    /// Struck-through text.
    pub strikethrough: bool,
}

impl Modifiers {
    fn set(&mut self, name: &str) -> bool {
        match name {
            "bold" => self.bold = true,
            "dim" => self.dim = true,
            "italic" => self.italic = true,
            "underline" => self.underline = true,
            "reversed" => self.reversed = true,
            "strikethrough" => self.strikethrough = true,
            _ => return false,
        }
        true
    }
}

/// The style for one theme key. `None` for a channel means "terminal default".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Style {
    fg: Option<Color>,
    bg: Option<Color>,
    modifiers: Modifiers,
}

impl Style {
    /// Foreground colour, if the theme sets one.
    #[must_use]
    pub fn fg(&self) -> Option<Color> {
        self.fg
    }

    /// Background colour, if the theme sets one.
    #[must_use]
    pub fn bg(&self) -> Option<Color> {
        self.bg
    }

    /// Text attributes.
    #[must_use]
    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }
}

/// A theme key from the ADR 0011 vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(missing_docs, reason = "variants mirror the key table in ADR 0011")]
pub enum Key {
    UiText,
    UiLinenr,
    UiSelection,
    UiSearchMatch,
    UiStatusline,
    UiStatuslineNormal,
    UiStatuslineSelect,
    UiStatuslineInput,
    UiStatuslineInfo,
    /// A banner that warns: the deleted-file row (ADR 0028).
    UiWarning,
    /// An affordance dimmer than text: `(z expand)` on a stub (ADR 0049).
    UiHint,
    /// The background of a pane's header rows (ADR 0059).
    UiHeader,
    UiSidebar,
    UiSidebarSelected,
    UiSidebarDir,
    UiPopup,
    UiPopupKey,
    /// The `Space` menu's and the right-click menu's surface (ADR 0056).
    UiMenu,
    UiPickerMatch,
    UiPickerSelected,
    DiffPlus,
    DiffDelta,
    DiffMinus,
    GitStaged,
    GitUnstaged,
    ThreadOpen,
    ThreadResolved,
    ThreadWaiting,
    ThreadLine,
    ThreadFocus,
    /// The background of a thread's stub and expanded rows (ADR 0049).
    ThreadInline,
    MarkupHeading,
    /// One heading level, 1 through 6; falls back to [`Key::MarkupHeading`].
    MarkupHeadingLevel(u8),
    MarkupRawInline,
    MarkupRawBlock,
    MarkupLink,
    MarkupList,
    MarkupQuote,
}

impl Key {
    const NAMED: [(&'static str, Self); 37] = [
        ("ui.text", Self::UiText),
        ("ui.linenr", Self::UiLinenr),
        ("ui.selection", Self::UiSelection),
        ("ui.search.match", Self::UiSearchMatch),
        ("ui.statusline", Self::UiStatusline),
        ("ui.statusline.normal", Self::UiStatuslineNormal),
        ("ui.statusline.select", Self::UiStatuslineSelect),
        ("ui.statusline.input", Self::UiStatuslineInput),
        ("ui.statusline.info", Self::UiStatuslineInfo),
        ("ui.warning", Self::UiWarning),
        ("ui.hint", Self::UiHint),
        ("ui.header", Self::UiHeader),
        ("ui.sidebar", Self::UiSidebar),
        ("ui.sidebar.selected", Self::UiSidebarSelected),
        ("ui.sidebar.dir", Self::UiSidebarDir),
        ("ui.popup", Self::UiPopup),
        ("ui.popup.key", Self::UiPopupKey),
        ("ui.menu", Self::UiMenu),
        ("ui.picker.match", Self::UiPickerMatch),
        ("ui.picker.selected", Self::UiPickerSelected),
        ("diff.plus", Self::DiffPlus),
        ("diff.delta", Self::DiffDelta),
        ("diff.minus", Self::DiffMinus),
        ("git.staged", Self::GitStaged),
        ("git.unstaged", Self::GitUnstaged),
        ("thread.open", Self::ThreadOpen),
        ("thread.resolved", Self::ThreadResolved),
        ("thread.waiting", Self::ThreadWaiting),
        ("thread.line", Self::ThreadLine),
        ("thread.focus", Self::ThreadFocus),
        ("thread.inline", Self::ThreadInline),
        ("markup.heading", Self::MarkupHeading),
        ("markup.raw.inline", Self::MarkupRawInline),
        ("markup.raw.block", Self::MarkupRawBlock),
        ("markup.link", Self::MarkupLink),
        ("markup.list", Self::MarkupList),
        ("markup.quote", Self::MarkupQuote),
    ];

    /// Every key a theme file may set, in documentation order.
    pub fn all() -> impl Iterator<Item = Self> {
        Self::NAMED
            .iter()
            .map(|(_, key)| *key)
            .chain((1..=6).map(Self::MarkupHeadingLevel))
    }

    /// The key as written in a theme file, e.g. `ui.linenr`.
    #[must_use]
    pub fn as_str(&self) -> String {
        match self {
            Self::MarkupHeadingLevel(level) => format!("markup.heading.{level}"),
            other => Self::NAMED
                .iter()
                .find(|(_, key)| key == other)
                .map_or_else(String::new, |(name, _)| (*name).to_owned()),
        }
    }

    fn parse(name: &str) -> Option<Self> {
        if let Some(level) = name.strip_prefix("markup.heading.") {
            return level
                .parse::<u8>()
                .ok()
                .filter(|level| (1..=6).contains(level))
                .map(Self::MarkupHeadingLevel);
        }
        Self::NAMED
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, key)| *key)
    }
}

/// A fully resolved theme: every key has a style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    name: String,
    styles: HashMap<Key, Style>,
    syntect: String,
}

impl Theme {
    /// Resolve `name` through the user's themes directory, then the built-ins.
    ///
    /// # Errors
    ///
    /// Returns [`ThemeError`] when no theme has that name, a file cannot be
    /// read, or a file in the inheritance chain is invalid.
    pub fn load(name: &str, dirs: &XdgDirs) -> Result<Self, ThemeError> {
        let themes_dir = dirs.themes_dir();
        Self::resolve(name, |name| read_user_theme(&themes_dir, name))
    }

    /// Resolve `name` using `user_source` for user files before built-ins.
    ///
    /// `user_source` returns `Ok(None)` when the user has no theme of that
    /// name; [`Theme::load`] backs it with the themes directory and tests
    /// back it with a map.
    ///
    /// # Errors
    ///
    /// Returns [`ThemeError`] when no theme has that name, `user_source`
    /// fails, or a file in the inheritance chain is invalid or cyclic.
    pub fn resolve(
        name: &str,
        user_source: impl Fn(&str) -> io::Result<Option<String>>,
    ) -> Result<Self, ThemeError> {
        let mut chain: Vec<ThemeFile> = Vec::new();
        let mut next = Some(name.to_owned());
        while let Some(current) = next.take() {
            if chain.iter().any(|file| file.name == current) {
                return Err(ThemeError::new(
                    &current,
                    None,
                    ErrorKind::Cycle(current.clone()),
                ));
            }
            let text = match user_source(&current) {
                Ok(Some(text)) => text,
                Ok(None) => builtin(&current)
                    .ok_or_else(|| ThemeError::new(&current, None, ErrorKind::NotFound))?
                    .to_owned(),
                Err(source) => {
                    return Err(ThemeError::new(&current, None, ErrorKind::Io(source)));
                }
            };
            let file = ThemeFile::parse(&current, &text)?;
            next.clone_from(&file.inherits);
            if next.is_none() && file.name != DEFAULT_THEME {
                next = Some(DEFAULT_THEME.to_owned());
            }
            chain.push(file);
        }

        // Apply root first so child files override their parents.
        let mut palette: HashMap<String, Option<Color>> = HashMap::new();
        let mut styles: HashMap<Key, Style> = HashMap::new();
        let mut syntect = None;
        for file in chain.iter().rev() {
            for (name, value) in &file.palette {
                let color = value
                    .resolve(&palette)
                    .map_err(|kind| ThemeError::new(&file.name, Some(value.location), kind))?;
                palette.insert(name.clone(), color);
            }
            for (key, raw) in &file.colors {
                let style = raw
                    .resolve(&palette)
                    .map_err(|kind| ThemeError::new(&file.name, Some(raw.location), kind))?;
                styles.insert(*key, style);
            }
            if let Some(name) = &file.syntect {
                syntect = Some(name.clone());
            }
        }
        for key in Key::all() {
            styles.entry(key).or_default();
        }
        Ok(Self {
            name: name.to_owned(),
            styles,
            syntect: syntect.unwrap_or_default(),
        })
    }

    /// The name this theme was resolved from.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The style for `key`. Heading levels fall back to `markup.heading`.
    #[must_use]
    pub fn style(&self, key: Key) -> Style {
        let direct = self.styles.get(&key).copied().unwrap_or_default();
        match key {
            Key::MarkupHeadingLevel(_) if direct == Style::default() => {
                self.style(Key::MarkupHeading)
            }
            _ => direct,
        }
    }

    /// The syntect theme name for code blocks; empty if no file set one.
    #[must_use]
    pub fn syntect(&self) -> &str {
        &self.syntect
    }
}

fn builtin(name: &str) -> Option<&'static str> {
    BUILTINS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, text)| *text)
}

fn read_user_theme(themes_dir: &Path, name: &str) -> io::Result<Option<String>> {
    // A name with a path separator would escape the themes directory.
    if name.contains(['/', '\\']) || name == ".." {
        return Ok(None);
    }
    match fs::read_to_string(themes_dir.join(format!("{name}.kdl"))) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Line and column (1-based) in a theme file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Location {
    line: usize,
    column: usize,
}

impl Location {
    /// 1-based line.
    #[must_use]
    pub fn line(&self) -> usize {
        self.line
    }

    /// 1-based column in characters.
    #[must_use]
    pub fn column(&self) -> usize {
        self.column
    }

    fn at(text: &str, char_offset: usize) -> Self {
        let mut line = 1;
        let mut column = 1;
        for ch in text.chars().take(char_offset) {
            if ch == '\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
        }
        Self { line, column }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

#[derive(Debug)]
enum ErrorKind {
    NotFound,
    Io(io::Error),
    Cycle(String),
    Syntax(String),
    UnknownNode(String),
    UnknownKey(String),
    UnknownProperty(String),
    UnknownModifier(String),
    UnknownColor(String),
    Expected(&'static str),
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "no theme with that name"),
            Self::Io(error) => write!(f, "cannot read theme file: {error}"),
            Self::Cycle(name) => write!(f, "`inherits` cycle through `{name}`"),
            Self::Syntax(message) => write!(f, "{message}"),
            Self::UnknownNode(name) => write!(f, "unknown node `{name}`"),
            Self::UnknownKey(name) => write!(f, "unknown theme key `{name}`"),
            Self::UnknownProperty(name) => write!(f, "unknown property `{name}`"),
            Self::UnknownModifier(name) => write!(f, "unknown modifier `{name}`"),
            Self::UnknownColor(name) => write!(f, "unknown colour `{name}`"),
            Self::Expected(what) => write!(f, "expected {what}"),
        }
    }
}

/// Why a theme could not be resolved, with the file and location when known.
#[derive(Debug)]
pub struct ThemeError {
    theme: String,
    location: Option<Location>,
    kind: ErrorKind,
}

impl ThemeError {
    fn new(theme: &str, location: Option<Location>, kind: ErrorKind) -> Self {
        Self {
            theme: theme.to_owned(),
            location,
            kind,
        }
    }

    /// The theme name the error occurred in (a parent, when inheriting).
    #[must_use]
    pub fn theme(&self) -> &str {
        &self.theme
    }

    /// Where in the file the error is, when it is a content error.
    #[must_use]
    pub fn location(&self) -> Option<Location> {
        self.location
    }

    /// True when no theme of the requested name exists anywhere.
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        matches!(self.kind, ErrorKind::NotFound)
    }
}

impl fmt::Display for ThemeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.location {
            Some(location) => write!(f, "theme `{}` at {location}: {}", self.theme, self.kind),
            None => write!(f, "theme `{}`: {}", self.theme, self.kind),
        }
    }
}

impl std::error::Error for ThemeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            ErrorKind::Io(error) => Some(error),
            _ => None,
        }
    }
}

/// A colour as written, resolved against the palette after inheritance.
#[derive(Debug, Clone)]
struct RawColor {
    text: String,
    location: Location,
}

impl RawColor {
    fn resolve(
        &self,
        palette: &HashMap<String, Option<Color>>,
    ) -> Result<Option<Color>, ErrorKind> {
        if self.text == "default" {
            return Ok(None);
        }
        if let Some(color) = palette.get(&self.text) {
            return Ok(*color);
        }
        if let Some(hex) = self.text.strip_prefix('#') {
            return parse_hex(hex)
                .map(Some)
                .ok_or_else(|| ErrorKind::UnknownColor(self.text.clone()));
        }
        AnsiColor::from_name(&self.text)
            .map(|c| Some(Color::Ansi(c)))
            .ok_or_else(|| ErrorKind::UnknownColor(self.text.clone()))
    }
}

fn parse_hex(hex: &str) -> Option<Color> {
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some(Color::Rgb(channel(0)?, channel(2)?, channel(4)?))
}

#[derive(Debug, Clone)]
struct RawStyle {
    fg: Option<RawColor>,
    bg: Option<RawColor>,
    modifiers: Modifiers,
    location: Location,
}

impl RawStyle {
    fn resolve(&self, palette: &HashMap<String, Option<Color>>) -> Result<Style, ErrorKind> {
        Ok(Style {
            fg: self
                .fg
                .as_ref()
                .map(|c| c.resolve(palette))
                .transpose()?
                .flatten(),
            bg: self
                .bg
                .as_ref()
                .map(|c| c.resolve(palette))
                .transpose()?
                .flatten(),
            modifiers: self.modifiers,
        })
    }
}

/// One parsed theme file before palette resolution and inheritance.
#[derive(Debug)]
struct ThemeFile {
    name: String,
    inherits: Option<String>,
    palette: Vec<(String, RawColor)>,
    colors: Vec<(Key, RawStyle)>,
    syntect: Option<String>,
}

impl ThemeFile {
    fn parse(name: &str, text: &str) -> Result<Self, ThemeError> {
        let err = |location: Option<Location>, kind| ThemeError::new(name, location, kind);
        let doc = KdlDocument::parse(text).map_err(|error| {
            let first = error.diagnostics.first();
            let location = first.map(|d| Location::at(text, d.span.offset()));
            let message = first
                .and_then(|d| d.message.clone())
                .unwrap_or_else(|| "invalid KDL".to_owned());
            err(location, ErrorKind::Syntax(message))
        })?;

        let mut file = Self {
            name: name.to_owned(),
            inherits: None,
            palette: Vec::new(),
            colors: Vec::new(),
            syntect: None,
        };
        for node in doc.nodes() {
            let at = |n: &KdlNode| Location::at(text, n.span().offset());
            match node.name().value() {
                "inherits" => file.inherits = Some(single_string(node, text, name)?),
                "palette" => {
                    for entry in children(node) {
                        let value = single_string(entry, text, name)?;
                        file.palette.push((
                            entry.name().value().to_owned(),
                            RawColor {
                                text: value,
                                location: at(entry),
                            },
                        ));
                    }
                }
                "code" => {
                    for entry in children(node) {
                        match entry.name().value() {
                            "syntect" => file.syntect = Some(single_string(entry, text, name)?),
                            other => {
                                return Err(err(
                                    Some(at(entry)),
                                    ErrorKind::UnknownNode(other.to_owned()),
                                ));
                            }
                        }
                    }
                }
                "colors" => {
                    for entry in children(node) {
                        let key_name = entry.name().value();
                        let key = Key::parse(key_name).ok_or_else(|| {
                            err(Some(at(entry)), ErrorKind::UnknownKey(key_name.to_owned()))
                        })?;
                        file.colors.push((key, parse_style(entry, text, name)?));
                    }
                }
                other => {
                    return Err(err(
                        Some(at(node)),
                        ErrorKind::UnknownNode(other.to_owned()),
                    ));
                }
            }
        }
        Ok(file)
    }
}

fn children(node: &KdlNode) -> impl Iterator<Item = &KdlNode> {
    node.children().into_iter().flat_map(KdlDocument::nodes)
}

fn entry_string<'a>(entry: &'a KdlEntry, text: &str, theme: &str) -> Result<&'a str, ThemeError> {
    entry.value().as_string().ok_or_else(|| {
        ThemeError::new(
            theme,
            Some(Location::at(text, entry.span().offset())),
            ErrorKind::Expected("a string"),
        )
    })
}

/// The one positional string argument of `node`, e.g. `inherits "x"`.
fn single_string(node: &KdlNode, text: &str, theme: &str) -> Result<String, ThemeError> {
    let location = Some(Location::at(text, node.span().offset()));
    let mut args = node.entries().iter().filter(|e| e.name().is_none());
    let (Some(first), None) = (args.next(), args.next()) else {
        return Err(ThemeError::new(
            theme,
            location,
            ErrorKind::Expected("exactly one string argument"),
        ));
    };
    if node.entries().iter().any(|e| e.name().is_some()) || node.children().is_some() {
        return Err(ThemeError::new(
            theme,
            location,
            ErrorKind::Expected("exactly one string argument"),
        ));
    }
    Ok(entry_string(first, text, theme)?.to_owned())
}

fn parse_style(node: &KdlNode, text: &str, theme: &str) -> Result<RawStyle, ThemeError> {
    let location = Location::at(text, node.span().offset());
    let mut style = RawStyle {
        fg: None,
        bg: None,
        modifiers: Modifiers::default(),
        location,
    };
    if node.children().is_some() {
        return Err(ThemeError::new(
            theme,
            Some(location),
            ErrorKind::Expected("fg, bg, and mods properties, not children"),
        ));
    }
    for entry in node.entries() {
        let entry_location = Location::at(text, entry.span().offset());
        let value = entry_string(entry, text, theme)?;
        let color = || RawColor {
            text: value.to_owned(),
            location: entry_location,
        };
        match entry.name().map(kdl::KdlIdentifier::value) {
            None | Some("fg") => style.fg = Some(color()),
            Some("bg") => style.bg = Some(color()),
            Some("mods") => {
                for word in value.split_whitespace() {
                    if !style.modifiers.set(word) {
                        return Err(ThemeError::new(
                            theme,
                            Some(entry_location),
                            ErrorKind::UnknownModifier(word.to_owned()),
                        ));
                    }
                }
            }
            Some(other) => {
                return Err(ThemeError::new(
                    theme,
                    Some(entry_location),
                    ErrorKind::UnknownProperty(other.to_owned()),
                ));
            }
        }
    }
    Ok(style)
}
