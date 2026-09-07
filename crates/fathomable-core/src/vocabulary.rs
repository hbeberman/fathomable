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

/// The `workspace` parameter every tool but `workspaces` takes: a
/// root, or a viewer name or id (ADR 0047).
pub const WORKSPACE: &str = "workspace";
/// The workspace `workspaces` pins (ADR 0055).
pub const SWITCH: &str = "switch";
/// A viewer name or id.
pub const VIEWER: &str = "viewer";
/// A harness session id.
pub const ID: &str = "id";
/// An agent type, `kind` in Rust.
pub const TYPE: &str = "type";
/// Whether `follow` ends the subscription instead (ADR 0055).
pub const END: &str = "end";
/// Whether `thread_watch` cancels the watch instead (ADR 0055).
pub const CANCEL: &str = "cancel";
/// Which threads `threads` lists: `open`, `pending`, `resolved`, or `all`.
pub const STATUS: &str = "status";
/// The field that marks a thread the user has the last word on, and the
/// `status` that lists only those (ADR 0055, ADR 0058).
pub const PENDING: &str = "pending";
/// The field naming the agent that has the last word on a thread (ADR 0058).
pub const ANSWERED: &str = "answered";
/// The field naming the worktree a thread is placed against when the
/// caller's does not reach it (ADR 0070).
pub const WORKTREE: &str = "worktree";
/// A workspace-relative file path.
pub const PATH: &str = "path";
/// A name to sign as, given once at `follow` (ADR 0058).
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
/// Several comments in one `thread_start` call (ADR 0061).
pub const COMMENTS: &str = "comments";
/// The thread a watch is on.
pub const ON: &str = "on";
/// What a watch waits for.
pub const WHEN: &str = "when";
/// The threads a fired watch reminds of.
pub const REMIND: &str = "remind";

/// The `when` of a watch that fires on a new message.
pub const WHEN_MESSAGE: &str = "message";
/// The `when` of a watch that fires on a resolve, and the `status` of
/// the threads the user closed.
pub const WHEN_RESOLVED: &str = "resolved";
/// The `status` of the threads still waiting: the default.
pub const STATUS_OPEN: &str = "open";
/// The `status` that lists open and resolved threads alike.
pub const STATUS_ALL: &str = "all";

/// List the known workspaces and their viewers; pin one with `switch`.
pub const WORKSPACES: Tool = Tool {
    name: "workspaces",
    params: &[SWITCH],
};
/// Show a file in the viewer.
pub const OPEN: Tool = Tool {
    name: "open",
    params: &[PATH, LINE, END_LINE, WORKSPACE, VIEWER],
};
/// Subscribe the session to the workspace, or end the subscription.
pub const FOLLOW: Tool = Tool {
    name: "follow",
    params: &[ID, TYPE, PERSONA, END, WORKSPACE],
};
/// Read threads: open by default, the ones waiting on the caller
/// flagged and delivered.
pub const THREADS: Tool = Tool {
    name: "threads",
    params: &[STATUS, PATH, SINCE, LIMIT, ID, WORKSPACE],
};
/// Answer one thread or several.
pub const THREAD_REPLY: Tool = Tool {
    name: "thread_reply",
    params: &[
        THREAD, BODY, RESOLVE, LINE, END_LINE, REPLIES, ID, WORKSPACE,
    ],
};
/// Start a thread, or several, on lines of a file (ADR 0061).
pub const THREAD_START: Tool = Tool {
    name: "thread_start",
    params: &[PATH, LINE, END_LINE, BODY, COMMENTS, ID, WORKSPACE],
};
/// Be woken when another thread moves, or cancel the watch.
pub const THREAD_WATCH: Tool = Tool {
    name: "thread_watch",
    params: &[ON, WHEN, REMIND, CANCEL, ID, WORKSPACE],
};

/// Every tool, in the order the guide lists them (ADR 0055, 0061).
pub const ALL: [Tool; 7] = [
    WORKSPACES,
    OPEN,
    FOLLOW,
    THREADS,
    THREAD_REPLY,
    THREAD_START,
    THREAD_WATCH,
];

/// Whether `ident` is a tool name, a parameter name, a `when` or
/// `status` value, or the `pending`, `answered`, or `worktree` field.
#[must_use]
pub fn is_known(ident: &str) -> bool {
    [
        WHEN_MESSAGE,
        WHEN_RESOLVED,
        STATUS_OPEN,
        STATUS_ALL,
        PENDING,
        ANSWERED,
        WORKTREE,
    ]
    .contains(&ident)
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
