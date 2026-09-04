// @okf-doc: /decisions/0008-configuration-format.md
//! The user configuration file, `config.kdl` (ADR 0008).
//!
//! A missing file is valid and yields [`Config::default`]. Every node the
//! file may contain is known; an unknown node is an error with a location
//! rather than being ignored, so typos surface immediately. `theme`, the
//! `jump` and `watch` blocks (ADR 0015, renamed by ADR 0047), the
//! `markdown` block (ADR 0016), the `viewer` block (ADR 0026), and the
//! `agents` block (ADR 0040) are understood; a `follow` block from before
//! the rename is an error that names where each setting went.
//!
//! # Examples
//!
//! ```
//! use fathomable_core::config::Config;
//!
//! let config = Config::parse("theme \"default-light\"").unwrap();
//! assert_eq!(config.theme(), Some("default-light"));
//! ```

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use kdl::{KdlDocument, KdlNode, KdlValue};

use crate::XdgDirs;

/// Settings read from `config.kdl`, with defaults for anything unset.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Config {
    theme: Option<String>,
    jump: JumpConfig,
    watch: WatchConfig,
    markdown: MarkdownConfig,
    viewer: ViewerConfig,
    rail: RailConfig,
    threads: ThreadsConfig,
    agents: AgentsConfig,
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

/// The `rail { ... }` block (ADR 0049): the left column that holds the
/// tree pane and the threads pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RailConfig {
    /// Columns the rail takes, clamped to a third of the terminal.
    pub width: usize,
    /// Rows the threads pane takes under the tree.
    pub split: usize,
}

impl Default for RailConfig {
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
    /// The command `Space w` runs to wake a subscriber, with `{id}` and
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

/// Where each setting of the retired `follow` block lives now (ADR 0047).
const FOLLOW_MOVED: [(&str, &str); 6] = [
    ("auto", "jump.auto"),
    ("jump-debounce", "jump.debounce"),
    ("toast", "jump.toast"),
    ("ignore", "watch.ignore"),
    ("hint-debounce", "watch.debounce"),
    ("seen-idle", "viewer.seen-idle"),
];

impl Config {
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
                    config.theme = Some(one_string(node, line)?.to_owned());
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
                "follow" => {
                    // The block before ADR 0047: say where each setting went.
                    let moved: Vec<String> = node
                        .children()
                        .map(|children| {
                            children
                                .nodes()
                                .iter()
                                .map(|child| {
                                    let name = child.name().value();
                                    FOLLOW_MOVED
                                        .iter()
                                        .find(|(old, _)| *old == name)
                                        .map_or_else(
                                            || format!("`{name}` is unknown"),
                                            |(_, new)| format!("`{name}` is now `{new}`"),
                                        )
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let detail = if moved.is_empty() {
                        "its settings are now `jump`, `watch`, and `viewer.seen-idle`".to_owned()
                    } else {
                        moved.join(", ")
                    };
                    return Err(ConfigError {
                        path: None,
                        line,
                        message: format!("`follow` moved: {detail}"),
                    });
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
                "rail" => {
                    let Some(children) = node.children() else {
                        return Err(ConfigError {
                            path: None,
                            line,
                            message: "`rail` takes a block of settings".to_owned(),
                        });
                    };
                    for child in children.nodes() {
                        let line = Some(line_of(child.span().offset()));
                        match child.name().value() {
                            "width" => config.rail.width = cells(child, line, "column count")?,
                            "split" => config.rail.split = cells(child, line, "row count")?,
                            other => {
                                return Err(ConfigError {
                                    path: None,
                                    line,
                                    message: format!("unknown rail setting `{other}`"),
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

    /// The theme name chosen by the config, if any.
    #[must_use]
    pub fn theme(&self) -> Option<&str> {
        self.theme.as_deref()
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

    /// The rail's width and split (ADR 0049).
    #[must_use]
    pub fn rail(&self) -> &RailConfig {
        &self.rail
    }

    /// How threads show in the text (ADR 0049).
    #[must_use]
    pub fn threads(&self) -> &ThreadsConfig {
        &self.threads
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
rail {
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
        assert_eq!(config.rail().width, 40);
        assert_eq!(config.rail().split, 12);
        assert!(!config.threads().stubs);
        assert!(config.threads().stubs_resolved);
        assert_eq!(Config::default().threads(), &ThreadsConfig::default());
        assert_eq!(Config::default().rail(), &RailConfig::default());
    }

    /// A `follow` block from before ADR 0047 is refused with the new home
    /// of each setting it holds.
    #[test]
    fn a_follow_block_names_where_each_setting_went() {
        let error = Config::parse(
            "follow {
    auto #true
    hint-debounce 50
    bogus 1
}",
        )
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
        assert_eq!(
            error,
            "config line 1: `follow` moved: `auto` is now `jump.auto`, `hint-debounce` is now `watch.debounce`, `bogus` is unknown"
        );
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
