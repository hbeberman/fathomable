// @okf-doc: /decisions/0047-one-vocabulary.md
//! The tool and parameter names the agent-facing text may use.
//!
//! Every string a model reads in MCP instructions and tool descriptions names
//! tools and
//! parameters through the constants here, and the `fathomable` crate
//! proves the table against its live tool schema (ADR 0043). This module
//! knows names only: no MCP types, so the core crate stays free of them.
//! Test-only prose checks live in `fathomable_testing::vocabulary`, keeping
//! the runtime vocabulary limited to names and the [`ALL`] table.

/// A tool with the names of its top-level parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tool {
    /// The name the MCP server registers.
    pub name: &'static str,
    /// The properties of its input schema, top level only.
    pub params: &'static [&'static str],
}

/// Which threads `threads` lists: `open`, `resolved`, or `all`.
pub const STATUS: &str = "status";
/// Exact thread ids to read, including resolved history.
pub const IDS: &str = "ids";
/// The field naming the worktree a thread is placed against when the
/// caller's does not reach it (ADR 0070).
pub const WORKTREE: &str = "worktree";
/// A repository-relative file path.
pub const PATH: &str = "path";
/// A first line.
pub const LINE: &str = "line";
/// A last line.
pub const END_LINE: &str = "end_line";
/// A Unix time lower bound.
pub const SINCE: &str = "since";
/// The deterministic position after which a page continues.
pub const AFTER: &str = "after";
/// A cap on how many threads come back.
pub const LIMIT: &str = "limit";
/// A reply's text.
pub const BODY: &str = "body";
/// Whether a reply also resolves.
pub const RESOLVE: &str = "resolve";
/// Several replies in one call.
pub const REPLIES: &str = "replies";
/// Several comments in one `thread_start` call (ADR 0061).
pub const COMMENTS: &str = "comments";
/// The `status` of the threads the user closed.
pub const WHEN_RESOLVED: &str = "resolved";
/// The `status` of the threads still waiting: the default.
pub const STATUS_OPEN: &str = "open";
/// The `status` that lists open and resolved threads alike.
pub const STATUS_ALL: &str = "all";

/// Read threads in the checkout bound when the MCP server starts.
pub const THREADS: Tool = Tool {
    name: "threads",
    params: &[STATUS, PATH, SINCE, AFTER, LIMIT, IDS],
};
/// Answer one or more threads.
pub const THREAD_REPLY: Tool = Tool {
    name: "thread_reply",
    params: &[REPLIES],
};
/// Start one or more threads on lines of a file.
pub const THREAD_START: Tool = Tool {
    name: "thread_start",
    params: &[COMMENTS],
};

/// Every tool, in the order the guide lists them.
pub const ALL: [Tool; 3] = [THREADS, THREAD_START, THREAD_REPLY];

#[cfg(test)]
mod tests {
    use super::ALL;

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
}
