// @okf-doc: /decisions/0008-configuration-format.md
//! The user configuration file, `config.kdl` (ADR 0008).
//!
//! A missing file is valid and yields [`Config::default`]. Every node the
//! file may contain is known; an unknown node is an error with a location
//! rather than being ignored, so typos surface immediately. `theme` and the
//! `follow` block (ADR 0015) are understood.
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
use crate::follow::Source;

/// Settings read from `config.kdl`, with defaults for anything unset.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Config {
    theme: Option<String>,
    follow: FollowConfig,
}

/// The `follow { ... }` block (ADR 0015).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowConfig {
    /// What counts as a change worth hinting.
    pub source: Source,
    /// Whether auto-jump starts enabled.
    pub auto: bool,
    /// Extra ignore globs, root-relative, on top of the tree's rules.
    pub ignore: Vec<String>,
    /// Quiet period before a burst of writes becomes one change.
    pub hint_debounce: Duration,
    /// Quiet period before auto-jump moves.
    pub jump_debounce: Duration,
    /// Idle time in a file before it counts as seen.
    pub seen_idle: Duration,
    /// How long a toast stays; zero disables toasts.
    pub toast: Duration,
}

impl Default for FollowConfig {
    fn default() -> Self {
        Self {
            source: Source::Workspace,
            auto: false,
            ignore: Vec::new(),
            hint_debounce: Duration::from_millis(300),
            jump_debounce: Duration::from_secs(1),
            seen_idle: Duration::from_secs(5),
            toast: Duration::from_secs(4),
        }
    }
}

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
                "follow" => {
                    let Some(children) = node.children() else {
                        return Err(ConfigError {
                            path: None,
                            line,
                            message: "`follow` takes a block of settings".to_owned(),
                        });
                    };
                    for child in children.nodes() {
                        let line = Some(line_of(child.span().offset()));
                        let follow = &mut config.follow;
                        match child.name().value() {
                            "source" => {
                                follow.source =
                                    one_string(child, line)?.parse().map_err(|error| {
                                        ConfigError {
                                            path: None,
                                            line,
                                            message: format!("{error}"),
                                        }
                                    })?;
                            }
                            "auto" => follow.auto = one_bool(child, line)?,
                            "ignore" => {
                                follow.ignore = child
                                    .entries()
                                    .iter()
                                    .filter(|e| e.name().is_none())
                                    .map(|e| {
                                        e.value().as_string().map(str::to_owned).ok_or_else(|| {
                                            ConfigError {
                                                path: None,
                                                line,
                                                message: "`ignore` takes strings".to_owned(),
                                            }
                                        })
                                    })
                                    .collect::<Result<_, _>>()?;
                            }
                            "hint-debounce" => follow.hint_debounce = millis(child, line)?,
                            "jump-debounce" => follow.jump_debounce = millis(child, line)?,
                            "seen-idle" => follow.seen_idle = millis(child, line)?,
                            "toast" => follow.toast = millis(child, line)?,
                            other => {
                                return Err(ConfigError {
                                    path: None,
                                    line,
                                    message: format!("unknown follow setting `{other}`"),
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

    /// The follow-mode settings (ADR 0015).
    #[must_use]
    pub fn follow(&self) -> &FollowConfig {
        &self.follow
    }
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
    one_arg(node, line, "millisecond count")?
        .as_integer()
        .and_then(|n| u64::try_from(n).ok())
        .map(Duration::from_millis)
        .ok_or_else(|| ConfigError {
            path: None,
            line,
            message: format!(
                "`{}` takes exactly one non-negative millisecond count",
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
    fn follow_block_parses_every_key() {
        let config = Config::parse(
            r#"
follow {
    source "followed"
    auto #true
    ignore "target/**" "*.lock"
    hint-debounce 50
    jump-debounce 2000
    seen-idle 10
    toast 0
}
"#,
        )
        .map_err(|e| e.to_string());
        let config = config.unwrap_or_default();
        let follow = config.follow();
        assert_eq!(follow.source, Source::Followed);
        assert!(follow.auto);
        assert_eq!(follow.ignore, ["target/**", "*.lock"]);
        assert_eq!(follow.hint_debounce, Duration::from_millis(50));
        assert_eq!(follow.jump_debounce, Duration::from_secs(2));
        assert_eq!(follow.seen_idle, Duration::from_millis(10));
        assert_eq!(follow.toast, Duration::ZERO);
    }

    #[test]
    fn follow_defaults_apply_per_key() {
        let config = Config::parse("follow { auto #true }").unwrap_or_default();
        assert!(config.follow().auto);
        assert_eq!(config.follow().source, Source::Workspace);
        assert_eq!(config.follow().toast, Duration::from_secs(4));
    }

    #[test]
    fn follow_errors_name_the_line() {
        let bad = [
            ("follow { source \"nope\" }", "unknown follow source"),
            ("follow { toast -1 }", "millisecond"),
            ("follow { auto \"yes\" }", "boolean"),
            ("follow { nope 1 }", "unknown follow setting"),
            ("follow \"x\"", "block"),
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
