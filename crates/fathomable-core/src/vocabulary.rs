// @okf-doc: /decisions/0047-one-vocabulary.md
//! The tool and parameter names the agent-facing text may use.
//!
//! Every string a model reads — the `hello` hook, the pending blob, the
//! MCP server instructions, the tool descriptions — names tools and
//! parameters through the constants here, and the `fathomable` crate
//! proves the table against its live tool schema (ADR 0043). This module
//! knows names only: no MCP types, so the core crate stays free of them.
//!
//! [`idents`] lists the backticked identifiers of a text and
//! [`is_known`] says whether one is in the table; tests on both sides of
//! the crate boundary use them to fail on a name that drifted.

/// A tool with the names of its top-level parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tool {
    /// The name the MCP server registers.
    pub name: &'static str,
    /// The properties of its input schema, top level only.
    pub params: &'static [&'static str],
}

/// The `session` parameter every tool but `session_list` takes.
pub const SESSION: &str = "session";
/// A viewer name or id.
pub const VIEWER: &str = "viewer";
/// A harness session id.
pub const ID: &str = "id";
/// An agent type, `kind` in Rust.
pub const TYPE: &str = "type";
/// The files a session follows.
pub const PATHS: &str = "paths";
/// A workspace-relative file path.
pub const PATH: &str = "path";
/// A name to sign as.
pub const PERSONA: &str = "persona";
/// A first line.
pub const LINE: &str = "line";
/// A last line.
pub const END_LINE: &str = "end_line";
/// A Unix time lower bound.
pub const SINCE: &str = "since";
/// A cap on how many threads come back.
pub const LIMIT: &str = "limit";
/// A thread id.
pub const THREAD: &str = "thread";
/// A reply's text.
pub const BODY: &str = "body";
/// Whether a reply also resolves.
pub const RESOLVE: &str = "resolve";
/// Several replies in one call.
pub const REPLIES: &str = "replies";
/// The thread a watch is on.
pub const ON: &str = "on";
/// What a watch waits for.
pub const WHEN: &str = "when";
/// The threads a fired watch reminds of.
pub const REMIND: &str = "remind";

/// The `when` of a watch that fires on a new message.
pub const WHEN_MESSAGE: &str = "message";
/// The `when` of a watch that fires on a resolve.
pub const WHEN_RESOLVED: &str = "resolved";

/// List the known workspaces and their viewers.
pub const SESSION_LIST: Tool = Tool {
    name: "session_list",
    params: &[],
};
/// Pin a workspace for later calls.
pub const SESSION_SWITCH: Tool = Tool {
    name: "session_switch",
    params: &[SESSION],
};
/// Show a file in the viewer.
pub const OPEN: Tool = Tool {
    name: "open",
    params: &[PATH, LINE, END_LINE, SESSION, VIEWER],
};
/// Name the files being edited and subscribe.
pub const FOLLOW: Tool = Tool {
    name: "follow",
    params: &[PATHS, ID, TYPE, PERSONA, SESSION, VIEWER],
};
/// End a subscription.
pub const UNFOLLOW: Tool = Tool {
    name: "unfollow",
    params: &[ID, SESSION],
};
/// Read threads.
pub const ANNOTATIONS_LIST: Tool = Tool {
    name: "annotations_list",
    params: &[SINCE, PATH, LIMIT, SESSION],
};
/// The threads waiting on a subscriber.
pub const THREADS_PENDING: Tool = Tool {
    name: "threads_pending",
    params: &[ID, LIMIT, SESSION],
};
/// Answer one thread or several.
pub const THREAD_REPLY: Tool = Tool {
    name: "thread_reply",
    params: &[
        THREAD, BODY, RESOLVE, LINE, END_LINE, REPLIES, PERSONA, ID, SESSION,
    ],
};
/// Be woken when another thread moves.
pub const THREAD_WATCH: Tool = Tool {
    name: "thread_watch",
    params: &[ON, WHEN, REMIND, ID, SESSION],
};
/// Cancel a watch.
pub const THREAD_UNWATCH: Tool = Tool {
    name: "thread_unwatch",
    params: &[ON, ID, SESSION],
};

/// Every tool, in the order the guide lists them.
pub const ALL: [Tool; 10] = [
    SESSION_LIST,
    SESSION_SWITCH,
    OPEN,
    FOLLOW,
    UNFOLLOW,
    ANNOTATIONS_LIST,
    THREADS_PENDING,
    THREAD_REPLY,
    THREAD_WATCH,
    THREAD_UNWATCH,
];

/// Whether `ident` is a tool name, a parameter name, or a `when` value.
#[must_use]
pub fn is_known(ident: &str) -> bool {
    ident == WHEN_MESSAGE
        || ident == WHEN_RESOLVED
        || ALL
            .iter()
            .any(|t| t.name == ident || t.params.contains(&ident))
}

/// The backticked identifiers of `text`, in order, repeats included.
///
/// A span is an identifier when it is one word of letters, digits,
/// underscores, dots, or dashes; a span with spaces or other punctuation
/// (a command line, a call shape) is skipped.
pub fn idents(text: &str) -> impl Iterator<Item = &str> {
    text.split('`').skip(1).step_by(2).filter(|span| {
        !span.is_empty()
            && span
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    })
}

#[cfg(test)]
mod tests {
    use super::{ALL, idents, is_known};

    #[test]
    fn tool_names_are_distinct_and_params_have_no_repeats() {
        for (i, tool) in ALL.iter().enumerate() {
            assert!(
                ALL[..i].iter().all(|t| t.name != tool.name),
                "{} listed twice",
                tool.name
            );
            for (j, param) in tool.params.iter().enumerate() {
                assert!(
                    !tool.params[..j].contains(param),
                    "{}.{param} listed twice",
                    tool.name
                );
            }
        }
    }

    #[test]
    fn idents_picks_single_words_only() {
        let text = "call `follow` with `id`, not `fathomable --mcp` or ``; `when` is `message`";
        let found: Vec<_> = idents(text).collect();
        assert_eq!(found, ["follow", "id", "when", "message"]);
        assert!(found.iter().all(|i| is_known(i)));
        assert!(!is_known("fathomable"));
    }
}
