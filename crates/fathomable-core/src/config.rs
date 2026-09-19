// @okf-doc: /decisions/0008-configuration-format.md
//! The user configuration file, `config.kdl` (ADR 0008).
//!
//! A missing file is valid and yields [`Config::default`]. Every node the
//! file may contain is known; an unknown node is an error with a location
//! rather than being ignored, so typos surface immediately. `theme`, the
//! `watch` block (ADR 0015, renamed by ADR 0047), the
//! `markdown` block (ADR 0016), the `viewer` block (ADR 0026), the
//! `layout` block (ADR 0081), and the `threads` block (ADR 0049) are
//! understood, along with the `diff` and `user` blocks.
//!
//! [`Config`] is [`Display`](fmt::Display): it writes the same KDL back
//! with every setting explained, which is what `--config-show` prints,
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
    watch: WatchConfig,
    markdown: MarkdownConfig,
    viewer: ViewerConfig,
    layout: LayoutConfig,
    threads: ThreadsConfig,
    diff: DiffConfig,
    user: UserConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: crate::theme::DEFAULT_THEME.to_owned(),
            watch: WatchConfig::default(),
            markdown: MarkdownConfig::default(),
            viewer: ViewerConfig::default(),
            layout: LayoutConfig::default(),
            threads: ThreadsConfig::default(),
            diff: DiffConfig::default(),
            user: UserConfig::default(),
        }
    }
}

/// How the workspace presents its selected diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DiffMode {
    /// Show Target content with comparison gutters and navigation.
    #[default]
    Standard,
    /// Show the selected comparison as a unified patch.
    Unified,
    /// Browse Target content without comparison presentation.
    Off,
}

impl fmt::Display for DiffMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Standard => "standard",
            Self::Unified => "unified",
            Self::Off => "off",
        })
    }
}

/// The `diff { ... }` block: startup diff presentation and comparison rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffConfig {
    /// The diff presentation used when the session starts.
    pub mode: DiffMode,
    /// Unchanged lines shown around each hunk; three, as `git diff`.
    pub context: usize,
    /// Whether the session starts with whitespace ignored (`Space d w`).
    pub ignore_whitespace: bool,
}

impl Default for DiffConfig {
    fn default() -> Self {
        Self {
            mode: DiffMode::default(),
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

/// The `layout { ... }` block: startup chrome and sidebar layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutConfig {
    /// Draw the persistent menu bar at startup.
    pub menu_bar: bool,
    /// The sidebar's startup state and geometry.
    pub sidebar: SidebarConfig,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            menu_bar: true,
            sidebar: SidebarConfig::default(),
        }
    }
}

/// The `layout.sidebar { ... }` block: the left column that holds the
/// files pane and the threads pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarConfig {
    /// Whether the configured pane composition is visible at startup.
    pub visible: bool,
    /// Whether the restore composition contains the files pane.
    pub files: bool,
    /// Whether the restore composition contains the threads pane.
    pub threads: bool,
    /// Columns the sidebar takes, clamped to a third of the terminal.
    pub width: usize,
    /// Rows the threads pane takes under the files pane.
    pub split: usize,
}

impl Default for SidebarConfig {
    fn default() -> Self {
        Self {
            visible: true,
            files: true,
            threads: true,
            width: 32,
            split: 8,
        }
    }
}

/// The `viewer { ... }` block (ADR 0026): how files are read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewerConfig {
    /// The largest text file the viewer reads, in MiB; larger ones show
    /// the file-info pane instead.
    pub max_file_size_mib: u64,
}

impl Default for ViewerConfig {
    fn default() -> Self {
        Self {
            max_file_size_mib: crate::content::DEFAULT_MAX_MIB,
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

/// The `watch { ... }` block (ADR 0015): what the file watcher reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchConfig {
    /// How long a toast stays; zero disables toasts.
    pub toast: Duration,
    /// Extra ignore globs, root-relative, on top of the tree's rules.
    pub ignore: Vec<String>,
    /// Quiet period for workspace and Git changes; thread refresh bypasses it.
    pub debounce: Duration,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            toast: Duration::from_secs(5),
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
                            "toast" => watch.toast = millis(child, line)?,
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
                            "mode" => {
                                config.diff.mode = match one_string(child, line)? {
                                    "standard" => DiffMode::Standard,
                                    "unified" => DiffMode::Unified,
                                    "off" => DiffMode::Off,
                                    value => {
                                        return Err(ConfigError {
                                            path: None,
                                            line,
                                            message: format!(
                                                "unknown diff mode `{value}`; expected `standard`, `unified`, or `off`"
                                            ),
                                        });
                                    }
                                };
                            }
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
                "layout" => {
                    let Some(children) = node.children() else {
                        return Err(ConfigError {
                            path: None,
                            line,
                            message: "`layout` takes a block of settings".to_owned(),
                        });
                    };
                    for child in children.nodes() {
                        let line = Some(line_of(child.span().offset()));
                        match child.name().value() {
                            "menu-bar" => config.layout.menu_bar = one_bool(child, line)?,
                            "sidebar" => {
                                let Some(settings) = child.children() else {
                                    return Err(ConfigError {
                                        path: None,
                                        line,
                                        message: "`layout.sidebar` takes a block of settings"
                                            .to_owned(),
                                    });
                                };
                                for setting in settings.nodes() {
                                    let line = Some(line_of(setting.span().offset()));
                                    let sidebar = &mut config.layout.sidebar;
                                    match setting.name().value() {
                                        "visible" => {
                                            sidebar.visible = one_bool(setting, line)?;
                                        }
                                        "files" => sidebar.files = one_bool(setting, line)?,
                                        "threads" => sidebar.threads = one_bool(setting, line)?,
                                        "width" => {
                                            sidebar.width = cells(setting, line, "column count")?;
                                        }
                                        "split" => {
                                            sidebar.split = cells(setting, line, "row count")?;
                                        }
                                        other => {
                                            return Err(ConfigError {
                                                path: None,
                                                line,
                                                message: format!(
                                                    "unknown layout.sidebar setting `{other}`"
                                                ),
                                            });
                                        }
                                    }
                                }
                            }
                            other => {
                                return Err(ConfigError {
                                    path: None,
                                    line,
                                    message: format!("unknown layout setting `{other}`"),
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

    /// File-watcher and toast settings, the `watch` block (ADR 0015).
    #[must_use]
    pub fn watch(&self) -> &WatchConfig {
        &self.watch
    }

    /// How files are read (ADR 0026).
    #[must_use]
    pub fn viewer(&self) -> &ViewerConfig {
        &self.viewer
    }

    /// Startup chrome and sidebar layout.
    #[must_use]
    pub fn layout(&self) -> &LayoutConfig {
        &self.layout
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

    /// The `user` block: how the person at the viewer is named (ADR 0058).
    #[must_use]
    pub fn user(&self) -> &UserConfig {
        &self.user
    }
}

/// The configuration as KDL, one node per block in the order the guide
/// lists them, every setting written out with a usage comment.
/// [`Config::parse`] reads it back to an equal value.
impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "\
theme {theme} // Built-in theme or a custom theme name from themes/.

watch {{
    toast {toast} // Toast duration in milliseconds; 0 disables toasts.
    ignore{ignore} // Extra root-relative globs excluded from live-change notifications.
    debounce {debounce} // Quiet period in milliseconds for workspace and Git changes, not threads.
}}

markdown {{
    extensions{extensions} // Markdown extensions, case-insensitive and without dots.
    names{names} // Extensionless Markdown filenames, case-insensitive.
}}

viewer {{
    max-file-size-mib {max_file_size_mib} // Largest text file to load, in MiB; larger files show file info.
}}

layout {{
    menu-bar #{menu_bar} // Show the menu bar at startup.
    sidebar {{
        visible #{visible} // Show the sidebar at startup.
        files #{files} // Include File list in the sidebar.
        threads #{threads} // Include Thread list in the sidebar.
        width {width} // Sidebar columns, capped at one third of the terminal.
        split {split} // Thread list rows when both sidebar panes are shown.
    }}
}}

threads {{
    stubs #{stubs} // Show inline thread summaries.
    stubs-resolved #{stubs_resolved} // Include resolved threads in inline summaries.
}}

diff {{
    mode {mode} // Startup presentation: \"standard\", \"unified\", or \"off\".
    context {context} // Unchanged lines shown around each diff hunk.
    ignore-whitespace #{ignore_whitespace} // Default only; saved comparisons keep their whitespace rule.
}}

user {{
    name {name} // Your non-empty display name for review comments.
}}
",
            theme = quoted(&self.theme),
            toast = self.watch.toast.as_millis(),
            ignore = words(&self.watch.ignore),
            debounce = self.watch.debounce.as_millis(),
            extensions = words(&self.markdown.extensions),
            names = words(&self.markdown.names),
            max_file_size_mib = self.viewer.max_file_size_mib,
            menu_bar = self.layout.menu_bar,
            visible = self.layout.sidebar.visible,
            files = self.layout.sidebar.files,
            threads = self.layout.sidebar.threads,
            width = self.layout.sidebar.width,
            split = self.layout.sidebar.split,
            stubs = self.threads.stubs,
            stubs_resolved = self.threads.stubs_resolved,
            mode = quoted(&self.diff.mode.to_string()),
            context = self.diff.context,
            ignore_whitespace = self.diff.ignore_whitespace,
            name = quoted(&self.user.name),
        )
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
    fn watch_and_viewer_blocks_parse_every_key() -> Result<(), ConfigError> {
        let config = Config::parse(
            r#"
watch {
    toast 0
    ignore "target/**" "*.lock"
    debounce 50
}
layout {
    menu-bar #false
    sidebar {
        visible #false
        files #true
        threads #false
        width 40
        split 12
    }
}
threads {
    stubs #false
    stubs-resolved #true
}
"#,
        )?;
        assert_eq!(config.watch().toast, Duration::ZERO);
        assert_eq!(config.watch().ignore, ["target/**", "*.lock"]);
        assert_eq!(config.watch().debounce, Duration::from_millis(50));
        assert!(!config.layout().menu_bar);
        assert!(!config.layout().sidebar.visible);
        assert!(config.layout().sidebar.files);
        assert!(!config.layout().sidebar.threads);
        assert_eq!(config.layout().sidebar.width, 40);
        assert_eq!(config.layout().sidebar.split, 12);
        assert!(!config.threads().stubs);
        assert!(config.threads().stubs_resolved);
        assert_eq!(Config::default().threads(), &ThreadsConfig::default());
        assert_eq!(Config::default().layout(), &LayoutConfig::default());
        Ok(())
    }

    #[test]
    fn layout_allows_an_empty_sidebar_composition() -> Result<(), ConfigError> {
        let config = Config::parse(
            "layout { menu-bar #true; sidebar { visible #true; files #false; threads #false } }",
        )?;
        assert!(config.layout().menu_bar);
        assert!(config.layout().sidebar.visible);
        assert!(!config.layout().sidebar.files);
        assert!(!config.layout().sidebar.threads);
        for (text, needle) in [
            ("sidebar { width 40 }", "unknown setting `sidebar`"),
            ("layout { nope #true }", "unknown layout setting"),
            (
                "layout { sidebar { nope #true } }",
                "unknown layout.sidebar setting",
            ),
            ("layout #true", "block"),
            ("layout { sidebar #true }", "block"),
        ] {
            let error = Config::parse(text)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert!(error.contains(needle), "{text}: {error}");
        }
        Ok(())
    }

    #[test]
    fn markdown_block_parses_and_matches() -> Result<(), ConfigError> {
        let config = Config::parse("markdown { extensions \".MD\" \"txt\"\n names \"Notes\" }")?;
        let markdown = config.markdown();
        assert_eq!(markdown.extensions, ["md", "txt"]);
        assert_eq!(markdown.names, ["notes"]);
        assert!(markdown.matches(Path::new("a/Notes.Md")));
        assert!(markdown.matches(Path::new("x.txt")));
        assert!(markdown.matches(Path::new("docs/NOTES")));
        assert!(!markdown.matches(Path::new("README")));
        assert!(!markdown.matches(Path::new("main.rs")));
        Ok(())
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

    /// The `user` block names the person at the viewer (ADR 0058): `User`
    /// unless set, never blank.
    #[test]
    fn user_block_names_the_user() -> Result<(), ConfigError> {
        assert_eq!(Config::default().user().name, "User");
        let config = Config::parse("user { name \"Henry\" }")?;
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
        Ok(())
    }

    /// The `diff` block sets the startup mode and comparison defaults;
    /// anything else is a typo.
    #[test]
    fn diff_block_sets_mode_context_and_whitespace() -> Result<(), ConfigError> {
        let config =
            Config::parse("diff { mode \"unified\"; context 5; ignore-whitespace #true }")?;
        assert_eq!(config.diff().mode, DiffMode::Unified);
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
        assert_eq!(Config::default().diff().mode, DiffMode::Standard);
        for (value, mode) in [
            ("standard", DiffMode::Standard),
            ("unified", DiffMode::Unified),
            ("off", DiffMode::Off),
        ] {
            let config = Config::parse(&format!("diff {{ mode \"{value}\" }}"))?;
            assert_eq!(config.diff().mode, mode);
        }
        for (text, needle) in [
            ("diff { width 5 }", "unknown diff setting `width`"),
            ("diff { context #true }", "context"),
            ("diff { mode \"side-by-side\" }", "unknown diff mode"),
            ("diff { mode \"Standard\" }", "unknown diff mode"),
            ("diff { mode #true }", "exactly one string"),
            ("diff { mode \"off\" \"unified\" }", "exactly one string"),
            ("diff 1", "block"),
        ] {
            let error = Config::parse(text)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert!(error.contains(needle), "{text}: {error}");
        }
        Ok(())
    }

    #[test]
    fn watch_errors_name_the_line_and_jump_is_unknown() {
        let bad = [
            ("watch { toast -1 }", "millisecond"),
            ("watch { nope 1 }", "unknown watch setting"),
            ("watch \"x\"", "block"),
            ("jump { toast 1 }", "unknown setting `jump`"),
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
