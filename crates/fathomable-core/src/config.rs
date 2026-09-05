// @okf-doc: /decisions/0008-configuration-format.md
//! The user configuration file, `config.kdl` (ADR 0008).
//!
//! A missing file is valid and yields [`Config::default`]. Every node the
//! file may contain is known; an unknown node is an error with a location
//! rather than being ignored, so typos surface immediately. `theme`, the
//! `jump` and `watch` blocks (ADR 0015, renamed by ADR 0047), the
//! `markdown` block (ADR 0016), the `viewer` block (ADR 0026), and the
//! `agents` block (ADR 0040), and the `sidebar` (ADR 0057; `rail` in 0049)
//! and `threads` blocks (ADR 0049) are understood.
//!
//! [`Config`] is [`Display`](fmt::Display): it writes the same KDL back
//! with every setting spelled out, which is what `--config-show` prints,
//! and [`Config::parse`] reads that text to an equal value.
//!
//! # Examples
//!
//! ```
//! use fathomable_core::config::Config;
//!
//! let config = Config::parse("theme \"default-light\"").unwrap();
//! assert_eq!(config.theme(), "default-light");
//! assert_eq!(Config::parse(&config.to_string()).unwrap(), config);
//! ```

use std::fmt::{self, Write as _};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use kdl::{KdlDocument, KdlNode, KdlValue};

use crate::XdgDirs;

/// Settings read from `config.kdl`, with defaults for anything unset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    theme: String,
    jump: JumpConfig,
    watch: WatchConfig,
    markdown: MarkdownConfig,
    viewer: ViewerConfig,
    sidebar: SidebarConfig,
    threads: ThreadsConfig,
    diff: DiffConfig,
    agents: AgentsConfig,
    user: UserConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: crate::theme::DEFAULT_THEME.to_owned(),
            jump: JumpConfig::default(),
            watch: WatchConfig::default(),
            markdown: MarkdownConfig::default(),
            viewer: ViewerConfig::default(),
            sidebar: SidebarConfig::default(),
            threads: ThreadsConfig::default(),
            diff: DiffConfig::default(),
            agents: AgentsConfig::default(),
            user: UserConfig::default(),
        }
    }
}

/// The `diff { ... }` block (ADR 0060): how the diff view compares and
/// lists two texts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffConfig {
    /// Unchanged lines shown around each hunk; three, as `git diff`.
    pub context: usize,
    /// Whether the session starts with whitespace ignored (`Space d w`).
    pub ignore_whitespace: bool,
}

impl Default for DiffConfig {
    fn default() -> Self {
        Self {
            context: crate::diff::DEFAULT_CONTEXT,
            ignore_whitespace: false,
        }
    }
}

impl DiffConfig {
    /// The whitespace rule and context the block asks for.
    #[must_use]
    pub fn compare(&self) -> crate::diff::Compare {
        crate::diff::Compare {
            context: self.context,
            whitespace: if self.ignore_whitespace {
                crate::diff::Whitespace::Ignore
            } else {
                crate::diff::Whitespace::Exact
            },
        }
    }
}

/// The `user { ... }` block (ADR 0058): how the person at the viewer is
/// named wherever a message's author is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserConfig {
    /// The name the user's messages carry; `User` by default.
    pub name: String,
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            name: crate::identity::DEFAULT_USER_NAME.to_owned(),
        }
    }
}

/// The `threads { ... }` block (ADR 0049): how threads show in the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadsConfig {
    /// Draw a stub under each thread's lines.
    pub stubs: bool,
    /// Give resolved threads a stub too.
    pub stubs_resolved: bool,
}

impl Default for ThreadsConfig {
    fn default() -> Self {
        Self {
            stubs: true,
            stubs_resolved: false,
        }
    }
}

/// The `sidebar { ... }` block (ADR 0049, named by ADR 0057): the left
/// column that holds the files pane and the threads pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarConfig {
    /// Columns the sidebar takes, clamped to a third of the terminal.
    pub width: usize,
    /// Rows the threads pane takes under the files pane.
    pub split: usize,
}

impl Default for SidebarConfig {
    fn default() -> Self {
        Self {
            width: 32,
            split: 8,
        }
    }
}

/// The `agents { ... }` block (ADR 0040): subscriptions and delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentsConfig {
    /// The agent types `follow` may declare.
    pub types: Vec<String>,
    /// Stop-hook checks between reminders about delivered, unanswered
    /// threads; zero never reminds.
    pub nag_after: u32,
    /// How long a subscription that is not heard from lives.
    pub expire_after: Duration,
    /// The longest hook prompt, in lines, before the rest is listed.
    pub max_lines: usize,
    /// The command `Space a w` runs to wake a subscriber, with `{id}` and
    /// `{prompt}` placeholders; `None` disables the key.
    pub wake: Option<String>,
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            types: ["coder", "reviewer", "planner"].map(str::to_owned).to_vec(),
            nag_after: 5,
            expire_after: Duration::from_hours(24),
            max_lines: 40,
            wake: None,
        }
    }
}

impl AgentsConfig {
    /// Whether `kind` is a declared agent type.
    #[must_use]
    pub fn allows(&self, kind: &str) -> bool {
        self.types.iter().any(|t| t == kind)
    }
}

/// The `viewer { ... }` block (ADR 0026): how files are read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewerConfig {
    /// The largest text file the viewer reads, in MiB; larger ones show
    /// the file-info pane instead.
    pub max_file_size_mib: u64,
    /// Idle time in a file before it counts as seen (ADR 0015).
    pub seen_idle: Duration,
}

impl Default for ViewerConfig {
    fn default() -> Self {
        Self {
            max_file_size_mib: crate::content::DEFAULT_MAX_MIB,
            seen_idle: Duration::from_secs(5),
        }
    }
}

impl ViewerConfig {
    /// The ceiling in bytes, saturating.
    #[must_use]
    pub fn max_file_bytes(&self) -> u64 {
        self.max_file_size_mib.saturating_mul(crate::content::MIB)
    }
}

/// The `markdown { ... }` block (ADR 0016): which files render as Markdown.
/// Everything else opens as highlighted source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownConfig {
    /// Extensions, lowercase, without the dot.
    pub extensions: Vec<String>,
    /// Extensionless file names that are prose (README, LICENSE), matched
    /// case-insensitively. Every other extensionless file is source.
    pub names: Vec<String>,
}

impl Default for MarkdownConfig {
    fn default() -> Self {
        Self {
            extensions: ["md", "markdown", "mdx"].map(str::to_owned).to_vec(),
            names: [
                "readme",
                "license",
                "licence",
                "copying",
                "changelog",
                "contributing",
                "authors",
                "notice",
            ]
            .map(str::to_owned)
            .to_vec(),
        }
    }
}

impl MarkdownConfig {
    /// Whether `path` should render as Markdown.
    #[must_use]
    pub fn matches(&self, path: &Path) -> bool {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some(ext) => {
                let ext = ext.to_ascii_lowercase();
                self.extensions.contains(&ext)
            }
            None => path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| self.names.contains(&name.to_ascii_lowercase())),
        }
    }
}

/// The `jump { ... }` block (ADR 0015): auto-jump and its toasts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JumpConfig {
    /// Whether auto-jump starts enabled.
    pub auto: bool,
    /// Quiet period before auto-jump moves.
    pub debounce: Duration,
    /// How long a toast stays; zero disables toasts.
    pub toast: Duration,
}

impl Default for JumpConfig {
    fn default() -> Self {
        Self {
            auto: false,
            debounce: Duration::from_secs(1),
            toast: Duration::from_secs(4),
        }
    }
}

/// The `watch { ... }` block (ADR 0015): what the file watcher reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchConfig {
    /// Extra ignore globs, root-relative, on top of the tree's rules.
    pub ignore: Vec<String>,
    /// Quiet period before a burst of writes becomes one change.
    pub debounce: Duration,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            ignore: Vec::new(),
            debounce: Duration::from_millis(300),
        }
    }
}

impl Config {
    /// The defaults, as if `config.kdl` were empty.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read `$XDG_CONFIG_HOME/fathomable/config.kdl`, or `path` if given.
    ///
    /// A missing file yields the defaults.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when the file exists but cannot be read or
    /// contains an unknown node or a malformed value.
    pub fn load(dirs: &XdgDirs, path: Option<&Path>) -> Result<Self, ConfigError> {
        let path = path.map_or_else(|| dirs.config_dir().join("config.kdl"), Path::to_path_buf);
        match fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text).map_err(|error| ConfigError {
                path: Some(path),
                ..error
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(ConfigError {
                path: Some(path),
                line: None,
                message: format!("cannot read config: {error}"),
            }),
        }
    }

    /// Parse config text.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] for invalid KDL, an unknown node, or a value
    /// of the wrong shape.
    #[expect(clippy::too_many_lines, reason = "one match arm per config block")]
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let line_of = |offset: usize| text.chars().take(offset).filter(|c| *c == '\n').count() + 1;
        let doc = KdlDocument::parse(text).map_err(|error| {
            let first = error.diagnostics.first();
            ConfigError {
                path: None,
                line: first.map(|d| line_of(d.span.offset())),
                message: first
                    .and_then(|d| d.message.clone())
                    .unwrap_or_else(|| "invalid KDL".to_owned()),
            }
        })?;
        let mut config = Self::default();
        for node in doc.nodes() {
            let line = Some(line_of(node.span().offset()));
            match node.name().value() {
                "theme" => {
                    one_string(node, line)?.clone_into(&mut config.theme);
                }
                "jump" => {
                    let Some(children) = node.children() else {
                        return Err(ConfigError {
                            path: None,
                            line,
                            message: "`jump` takes a block of settings".to_owned(),
                        });
                    };
                    for child in children.nodes() {
                        let line = Some(line_of(child.span().offset()));
                        let jump = &mut config.jump;
                        match child.name().value() {
                            "auto" => jump.auto = one_bool(child, line)?,
                            "debounce" => jump.debounce = millis(child, line)?,
                            "toast" => jump.toast = millis(child, line)?,
                            other => {
                                return Err(ConfigError {
                                    path: None,
                                    line,
                                    message: format!("unknown jump setting `{other}`"),
                                });
                            }
                        }
                    }
                }
                "watch" => {
                    let Some(children) = node.children() else {
                        return Err(ConfigError {
                            path: None,
                            line,
                            message: "`watch` takes a block of settings".to_owned(),
                        });
                    };
                    for child in children.nodes() {
                        let line = Some(line_of(child.span().offset()));
                        let watch = &mut config.watch;
                        match child.name().value() {
                            "ignore" => watch.ignore = strings(child, line, "ignore")?,
                            "debounce" => watch.debounce = millis(child, line)?,
                            other => {
                                return Err(ConfigError {
                                    path: None,
                                    line,
                                    message: format!("unknown watch setting `{other}`"),
                                });
                            }
                        }
                    }
                }
                "markdown" => {
                    let Some(children) = node.children() else {
                        return Err(ConfigError {
                            path: None,
                            line,
                            message: "`markdown` takes a block of settings".to_owned(),
                        });
                    };
                    for child in children.nodes() {
                        let line = Some(line_of(child.span().offset()));
                        let markdown = &mut config.markdown;
                        match child.name().value() {
                            "extensions" => {
                                markdown.extensions = strings(child, line, "extensions")?
                                    .into_iter()
                                    .map(|ext| ext.trim_start_matches('.').to_ascii_lowercase())
                                    .collect();
                            }
                            "names" => {
                                markdown.names = strings(child, line, "names")?
                                    .into_iter()
                                    .map(|name| name.to_ascii_lowercase())
                                    .collect();
                            }
                            other => {
                                return Err(ConfigError {
                                    path: None,
                                    line,
                                    message: format!("unknown markdown setting `{other}`"),
                                });
                            }
                        }
                    }
                }
                "agents" => {
                    let Some(children) = node.children() else {
                        return Err(ConfigError {
                            path: None,
                            line,
                            message: "`agents` takes a block of settings".to_owned(),
                        });
                    };
                    for child in children.nodes() {
                        let line = Some(line_of(child.span().offset()));
                        let agents = &mut config.agents;
                        match child.name().value() {
                            "types" => agents.types = strings(child, line, "types")?,
                            "nag-after" => {
                                agents.nag_after =
                                    u32::try_from(count(child, line, "count")?).unwrap_or(u32::MAX);
                            }
                            "expire-after" => {
                                agents.expire_after = Duration::from_secs(
                                    count(child, line, "hour count")?.saturating_mul(3600),
                                );
                            }
                            "max-lines" => {
                                agents.max_lines =
                                    usize::try_from(count(child, line, "line count")?)
                                        .unwrap_or(usize::MAX);
                            }
                            "wake" => {
                                let command = one_string(child, line)?;
                                agents.wake = (!command.is_empty()).then(|| command.to_owned());
                            }
                            other => {
                                return Err(ConfigError {
                                    path: None,
                                    line,
                                    message: format!("unknown agents setting `{other}`"),
                                });
                            }
                        }
                    }
                }
                "user" => {
                    let Some(children) = node.children() else {
                        return Err(ConfigError {
                            path: None,
                            line,
                            message: "`user` takes a block of settings".to_owned(),
                        });
                    };
                    for child in children.nodes() {
                        let line = Some(line_of(child.span().offset()));
                        match child.name().value() {
                            "name" => {
                                let name = one_string(child, line)?.trim();
                                if name.is_empty() {
                                    return Err(ConfigError {
                                        path: None,
                                        line,
                                        message: "`user.name` must not be empty".to_owned(),
                                    });
                                }
                                name.clone_into(&mut config.user.name);
                            }
                            other => {
                                return Err(ConfigError {
                                    path: None,
                                    line,
                                    message: format!("unknown user setting `{other}`"),
                                });
                            }
                        }
                    }
                }
                "threads" => {
                    let Some(children) = node.children() else {
                        return Err(ConfigError {
                            path: None,
                            line,
                            message: "`threads` takes a block of settings".to_owned(),
                        });
                    };
                    for child in children.nodes() {
                        let line = Some(line_of(child.span().offset()));
                        match child.name().value() {
                            "stubs" => config.threads.stubs = one_bool(child, line)?,
                            "stubs-resolved" => {
                                config.threads.stubs_resolved = one_bool(child, line)?;
                            }
                            other => {
                                return Err(ConfigError {
                                    path: None,
                                    line,
                                    message: format!("unknown threads setting `{other}`"),
                                });
                            }
                        }
                    }
                }
                "diff" => {
                    let Some(children) = node.children() else {
                        return Err(ConfigError {
                            path: None,
                            line,
                            message: "`diff` takes a block of settings".to_owned(),
                        });
                    };
                    for child in children.nodes() {
                        let line = Some(line_of(child.span().offset()));
                        match child.name().value() {
                            "context" => {
                                config.diff.context = cells(child, line, "context")?;
                            }
                            "ignore-whitespace" => {
                                config.diff.ignore_whitespace = one_bool(child, line)?;
                            }
                            other => {
                                return Err(ConfigError {
                                    path: None,
                                    line,
                                    message: format!("unknown diff setting `{other}`"),
                                });
                            }
                        }
                    }
                }
                "sidebar" => {
                    let Some(children) = node.children() else {
                        return Err(ConfigError {
                            path: None,
                            line,
                            message: "`sidebar` takes a block of settings".to_owned(),
                        });
                    };
                    for child in children.nodes() {
                        let line = Some(line_of(child.span().offset()));
                        match child.name().value() {
                            "width" => config.sidebar.width = cells(child, line, "column count")?,
                            "split" => config.sidebar.split = cells(child, line, "row count")?,
                            other => {
                                return Err(ConfigError {
                                    path: None,
                                    line,
                                    message: format!("unknown sidebar setting `{other}`"),
                                });
                            }
                        }
                    }
                }
                "viewer" => {
                    let Some(children) = node.children() else {
                        return Err(ConfigError {
                            path: None,
                            line,
                            message: "`viewer` takes a block of settings".to_owned(),
                        });
                    };
                    for child in children.nodes() {
                        let line = Some(line_of(child.span().offset()));
                        match child.name().value() {
                            "max-file-size-mib" => {
                                config.viewer.max_file_size_mib = count(child, line, "MiB count")?;
                            }
                            "seen-idle" => config.viewer.seen_idle = millis(child, line)?,
                            other => {
                                return Err(ConfigError {
                                    path: None,
                                    line,
                                    message: format!("unknown viewer setting `{other}`"),
                                });
                            }
                        }
                    }
                }
                other => {
                    return Err(ConfigError {
                        path: None,
                        line,
                        message: format!("unknown setting `{other}`"),
                    });
                }
            }
        }
        Ok(config)
    }

    /// The theme name: the config's, else the built-in default.
    #[must_use]
    pub fn theme(&self) -> &str {
        &self.theme
    }

    /// Choose the theme, as `--theme` does over the file's.
    pub fn set_theme(&mut self, name: impl Into<String>) {
        self.theme = name.into();
    }

    /// Auto-jump settings, the `jump` block (ADR 0015).
    #[must_use]
    pub fn jump(&self) -> &JumpConfig {
        &self.jump
    }

    /// File-watcher settings, the `watch` block (ADR 0015).
    #[must_use]
    pub fn watch(&self) -> &WatchConfig {
        &self.watch
    }

    /// How files are read (ADR 0026).
    #[must_use]
    pub fn viewer(&self) -> &ViewerConfig {
        &self.viewer
    }

    /// The sidebar's width and split (ADR 0049, ADR 0057).
    #[must_use]
    pub fn sidebar(&self) -> &SidebarConfig {
        &self.sidebar
    }

    /// How threads show in the text (ADR 0049).
    #[must_use]
    pub fn threads(&self) -> &ThreadsConfig {
        &self.threads
    }

    /// The `diff` block (ADR 0060).
    #[must_use]
    pub fn diff(&self) -> &DiffConfig {
        &self.diff
    }

    /// Which files render as Markdown (ADR 0016).
    #[must_use]
    pub fn markdown(&self) -> &MarkdownConfig {
        &self.markdown
    }

    /// Subscriptions and delivery (ADR 0040).
    #[must_use]
    pub fn agents(&self) -> &AgentsConfig {
        &self.agents
    }

    /// The `user` block: how the person at the viewer is named (ADR 0058).
    #[must_use]
    pub fn user(&self) -> &UserConfig {
        &self.user
    }
}

/// The configuration as KDL, one node per block in the order the guide
/// lists them, every setting written out. [`Config::parse`] reads it back
/// to an equal value.
impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "theme {}", quoted(&self.theme))?;
        let jump = &self.jump;
        writeln!(f, "\njump {{")?;
        writeln!(f, "    auto #{}", jump.auto)?;
        writeln!(f, "    debounce {}", jump.debounce.as_millis())?;
        writeln!(f, "    toast {}", jump.toast.as_millis())?;
        writeln!(f, "}}")?;
        let watch = &self.watch;
        writeln!(f, "\nwatch {{")?;
        writeln!(f, "    ignore{}", words(&watch.ignore))?;
        writeln!(f, "    debounce {}", watch.debounce.as_millis())?;
        writeln!(f, "}}")?;
        let markdown = &self.markdown;
        writeln!(f, "\nmarkdown {{")?;
        writeln!(f, "    extensions{}", words(&markdown.extensions))?;
        writeln!(f, "    names{}", words(&markdown.names))?;
        writeln!(f, "}}")?;
        let viewer = &self.viewer;
        writeln!(f, "\nviewer {{")?;
        writeln!(f, "    max-file-size-mib {}", viewer.max_file_size_mib)?;
        writeln!(f, "    seen-idle {}", viewer.seen_idle.as_millis())?;
        writeln!(f, "}}")?;
        let sidebar = &self.sidebar;
        writeln!(f, "\nsidebar {{")?;
        writeln!(f, "    width {}", sidebar.width)?;
        writeln!(f, "    split {}", sidebar.split)?;
        writeln!(f, "}}")?;
        let threads = &self.threads;
        writeln!(f, "\nthreads {{")?;
        writeln!(f, "    stubs #{}", threads.stubs)?;
        writeln!(f, "    stubs-resolved #{}", threads.stubs_resolved)?;
        writeln!(f, "}}")?;
        let diff = &self.diff;
        writeln!(f, "\ndiff {{")?;
        writeln!(f, "    context {}", diff.context)?;
        writeln!(f, "    ignore-whitespace #{}", diff.ignore_whitespace)?;
        writeln!(f, "}}")?;
        let agents = &self.agents;
        writeln!(f, "\nagents {{")?;
        writeln!(f, "    types{}", words(&agents.types))?;
        writeln!(f, "    nag-after {}", agents.nag_after)?;
        writeln!(
            f,
            "    expire-after {}",
            agents.expire_after.as_secs() / 3600
        )?;
        writeln!(f, "    max-lines {}", agents.max_lines)?;
        writeln!(
            f,
            "    wake {}",
            quoted(agents.wake.as_deref().unwrap_or_default())
        )?;
        writeln!(f, "}}")?;
        writeln!(f, "\nuser {{")?;
        writeln!(f, "    name {}", quoted(&self.user.name))?;
        writeln!(f, "}}")
    }
}

/// `text` as a quoted KDL string: `"`, `\\`, and control characters
/// escaped, everything else literal.
fn quoted(text: &str) -> impl fmt::Display {
    struct Quoted<'a>(&'a str);

    impl fmt::Display for Quoted<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_char('"')?;
            for c in self.0.chars() {
                match c {
                    '"' => f.write_str("\\\"")?,
                    '\\' => f.write_str("\\\\")?,
                    '\n' => f.write_str("\\n")?,
                    '\r' => f.write_str("\\r")?,
                    '\t' => f.write_str("\\t")?,
                    c if c.is_control() => write!(f, "\\u{{{:x}}}", u32::from(c))?,
                    c => f.write_char(c)?,
                }
            }
            f.write_char('"')
        }
    }

    Quoted(text)
}

/// `items` as quoted KDL strings, each after a space; empty for none.
fn words(items: &[String]) -> impl fmt::Display {
    struct Words<'a>(&'a [String]);

    impl fmt::Display for Words<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            for item in self.0 {
                write!(f, " {}", quoted(item))?;
            }
            Ok(())
        }
    }

    Words(items)
}

/// Every positional string argument of `node`.
fn strings(node: &KdlNode, line: Option<usize>, name: &str) -> Result<Vec<String>, ConfigError> {
    node.entries()
        .iter()
        .filter(|e| e.name().is_none())
        .map(|e| {
            e.value()
                .as_string()
                .map(str::to_owned)
                .ok_or_else(|| ConfigError {
                    path: None,
                    line,
                    message: format!("`{name}` takes strings"),
                })
        })
        .collect()
}

/// The single positional argument of `node`, if it is exactly one.
fn one_arg<'a>(
    node: &'a KdlNode,
    line: Option<usize>,
    what: &str,
) -> Result<&'a KdlValue, ConfigError> {
    let mut args = node.entries().iter().filter(|e| e.name().is_none());
    match (args.next(), args.next()) {
        (Some(entry), None) => Ok(entry.value()),
        _ => Err(ConfigError {
            path: None,
            line,
            message: format!("`{}` takes exactly one {what}", node.name().value()),
        }),
    }
}

fn one_string(node: &KdlNode, line: Option<usize>) -> Result<&str, ConfigError> {
    one_arg(node, line, "string")?
        .as_string()
        .ok_or_else(|| ConfigError {
            path: None,
            line,
            message: format!("`{}` takes exactly one string", node.name().value()),
        })
}

fn one_bool(node: &KdlNode, line: Option<usize>) -> Result<bool, ConfigError> {
    one_arg(node, line, "boolean")?
        .as_bool()
        .ok_or_else(|| ConfigError {
            path: None,
            line,
            message: format!("`{}` takes exactly one boolean", node.name().value()),
        })
}

/// A non-negative millisecond count as a duration.
fn millis(node: &KdlNode, line: Option<usize>) -> Result<Duration, ConfigError> {
    count(node, line, "millisecond count").map(Duration::from_millis)
}

/// A cell count that fits `usize`, described as `what` in the error.
fn cells(node: &KdlNode, line: Option<usize>, what: &str) -> Result<usize, ConfigError> {
    let n = count(node, line, what)?;
    usize::try_from(n).map_err(|_overflow| ConfigError {
        path: None,
        line,
        message: format!("`{}` is too large a {what}", node.name().value()),
    })
}

/// A non-negative integer, described as `what` in the error.
fn count(node: &KdlNode, line: Option<usize>, what: &str) -> Result<u64, ConfigError> {
    one_arg(node, line, what)?
        .as_integer()
        .and_then(|n| u64::try_from(n).ok())
        .ok_or_else(|| ConfigError {
            path: None,
            line,
            message: format!(
                "`{}` takes exactly one non-negative {what}",
                node.name().value()
            ),
        })
}

/// Why the configuration could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    path: Option<PathBuf>,
    line: Option<usize>,
    message: String,
}

impl ConfigError {
    /// The 1-based line the error is on, for content errors.
    #[must_use]
    pub fn line(&self) -> Option<usize> {
        self.line
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.path, self.line) {
            (Some(path), Some(line)) => write!(f, "{}:{line}: {}", path.display(), self.message),
            (Some(path), None) => write!(f, "{}: {}", path.display(), self.message),
            (None, Some(line)) => write!(f, "config line {line}: {}", self.message),
            (None, None) => write!(f, "config: {}", self.message),
        }
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jump_watch_and_viewer_blocks_parse_every_key() {
        let config = Config::parse(
            r#"
jump {
    auto #true
    debounce 2000
    toast 0
}
watch {
    ignore "target/**" "*.lock"
    debounce 50
}
viewer {
    seen-idle 10
}
sidebar {
    width 40
    split 12
}
threads {
    stubs #false
    stubs-resolved #true
}
"#,
        )
        .map_err(|e| e.to_string());
        let config = config.unwrap_or_default();
        assert!(config.jump().auto);
        assert_eq!(config.jump().debounce, Duration::from_secs(2));
        assert_eq!(config.jump().toast, Duration::ZERO);
        assert_eq!(config.watch().ignore, ["target/**", "*.lock"]);
        assert_eq!(config.watch().debounce, Duration::from_millis(50));
        assert_eq!(config.viewer().seen_idle, Duration::from_millis(10));
        assert_eq!(config.sidebar().width, 40);
        assert_eq!(config.sidebar().split, 12);
        assert!(!config.threads().stubs);
        assert!(config.threads().stubs_resolved);
        assert_eq!(Config::default().threads(), &ThreadsConfig::default());
        assert_eq!(Config::default().sidebar(), &SidebarConfig::default());
    }

    #[test]
    fn markdown_block_parses_and_matches() {
        let config = Config::parse("markdown { extensions \".MD\" \"txt\"\n names \"Notes\" }")
            .unwrap_or_default();
        let markdown = config.markdown();
        assert_eq!(markdown.extensions, ["md", "txt"]);
        assert_eq!(markdown.names, ["notes"]);
        assert!(markdown.matches(Path::new("a/Notes.Md")));
        assert!(markdown.matches(Path::new("x.txt")));
        assert!(markdown.matches(Path::new("docs/NOTES")));
        assert!(!markdown.matches(Path::new("README")));
        assert!(!markdown.matches(Path::new("main.rs")));
    }

    #[test]
    fn markdown_defaults_cover_readme_and_md() {
        let markdown = MarkdownConfig::default();
        assert!(markdown.matches(Path::new("README")));
        assert!(markdown.matches(Path::new("License")));
        assert!(markdown.matches(Path::new("docs/guide.md")));
        assert!(markdown.matches(Path::new("x.mdx")));
        assert!(!markdown.matches(Path::new("Cargo.toml")));
        assert!(!markdown.matches(Path::new("src/main.rs")));
        assert!(
            !markdown.matches(Path::new("justfile")) && !markdown.matches(Path::new("Makefile")),
            "unlisted extensionless files are source"
        );
        assert!(
            !markdown.matches(Path::new(".gitignore")),
            "dotfiles are not prose"
        );
        assert!(!markdown.matches(Path::new("a/.env")));
    }

    #[test]
    fn markdown_errors_name_the_line() {
        for (text, needle) in [
            ("markdown { nope 1 }", "unknown markdown setting"),
            ("markdown { extensions 1 }", "takes strings"),
            ("markdown \"x\"", "block"),
        ] {
            let error = Config::parse(text)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert!(error.contains(needle), "{text}: {error}");
        }
    }

    #[test]
    fn agents_block_parses_every_key() {
        let config = Config::parse(
            "agents {\n types \"coder\" \"qa\"\n nag-after 0\n expire-after 2\n max-lines 10\n wake \"claude -r {id} {prompt}\"\n}",
        )
        .unwrap_or_default();
        let agents = config.agents();
        assert_eq!(agents.types, ["coder", "qa"]);
        assert!(agents.allows("qa") && !agents.allows("planner"));
        assert_eq!(agents.nag_after, 0);
        assert_eq!(agents.expire_after, Duration::from_hours(2));
        assert_eq!(agents.max_lines, 10);
        assert_eq!(agents.wake.as_deref(), Some("claude -r {id} {prompt}"));
        let defaults = Config::parse("agents { wake \"\" }").unwrap_or_default();
        assert_eq!(defaults.agents().wake, None);
        assert!(defaults.agents().allows("planner"));
        assert_eq!(defaults.agents().nag_after, 5);
        for (text, needle) in [
            ("agents { nope 1 }", "unknown agents setting"),
            ("agents { types 1 }", "takes strings"),
            ("agents { nag-after -1 }", "non-negative"),
            ("agents \"x\"", "block"),
        ] {
            let error = Config::parse(text)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert!(error.contains(needle), "{text}: {error}");
        }
    }

    /// The `user` block names the person at the viewer (ADR 0058): `User`
    /// unless set, never blank.
    #[test]
    fn user_block_names_the_user() {
        assert_eq!(Config::default().user().name, "User");
        let config = Config::parse("user { name \"Henry\" }").unwrap_or_default();
        assert_eq!(config.user().name, "Henry");
        for (text, needle) in [
            ("user { name \"  \" }", "must not be empty"),
            ("user { nope 1 }", "unknown user setting"),
            ("user \"x\"", "block"),
        ] {
            let error = Config::parse(text)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert!(error.contains(needle), "{text}: {error}");
        }
    }

    /// The `diff` block (ADR 0060) sets the context lines and whether the
    /// session starts with whitespace ignored; anything else is a typo.
    #[test]
    fn diff_block_sets_context_and_whitespace() {
        let config =
            Config::parse("diff { context 5; ignore-whitespace #true }").unwrap_or_default();
        assert_eq!(config.diff().context, 5);
        assert!(config.diff().ignore_whitespace);
        assert_eq!(
            config.diff().compare(),
            crate::diff::Compare {
                context: 5,
                whitespace: crate::diff::Whitespace::Ignore,
            }
        );
        assert_eq!(
            Config::default().diff().compare(),
            crate::diff::Compare::default()
        );
        for (text, needle) in [
            ("diff { width 5 }", "unknown diff setting `width`"),
            ("diff { context #true }", "context"),
            ("diff 1", "block"),
        ] {
            let error = Config::parse(text)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert!(error.contains(needle), "{text}: {error}");
        }
    }

    #[test]
    fn jump_defaults_apply_per_key() {
        let config = Config::parse("jump { auto #true }").unwrap_or_default();
        assert!(config.jump().auto);
        assert_eq!(config.jump().toast, Duration::from_secs(4));
    }

    #[test]
    fn jump_and_watch_errors_name_the_line() {
        let bad = [
            ("jump { toast -1 }", "millisecond"),
            ("jump { auto \"yes\" }", "boolean"),
            ("jump { nope 1 }", "unknown jump setting"),
            ("watch { nope 1 }", "unknown watch setting"),
            ("jump \"x\"", "block"),
        ];
        for (text, needle) in bad {
            let error = Config::parse(text)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert!(error.contains(needle), "{text}: {error}");
            assert!(error.contains("line 1"), "{text}: {error}");
        }
    }
}
