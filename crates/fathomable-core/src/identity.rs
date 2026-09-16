// @okf-doc: /decisions/0058-the-user-has-the-last-word.md
//! Who is who on a thread: stable harness labels and the user's name.
//!
//! An MCP author carries the harness label returned by [`harness_name`]
//! and its harness-qualified chat id. The user is named by
//! `user { name }` in the config, or [`DEFAULT_USER_NAME`].
//!
//! # Examples
//!
//! ```
//! use fathomable_core::identity::harness_name;
//!
//! assert_eq!(harness_name("copilot-cli"), Some("Copilot"));
//! ```

/// The user's name when the config sets none.
pub const DEFAULT_USER_NAME: &str = "User";

/// The harness name for the MCP client string `client`, when it is a
/// known one: the string a harness reports changes between its releases
/// (`github-copilot-developer` became `copilot-cli`), the name does not.
#[must_use]
pub fn harness_name(client: &str) -> Option<&'static str> {
    match client {
        "claude-code" => Some("Claude"),
        "copilot" | "copilot-cli" | "github-copilot-developer" => Some("Copilot"),
        "codex-mcp-client" | "codex" => Some("Codex"),
        "Visual Studio Code" | "Visual Studio Code - Insiders" | "Code - OSS" => Some("VS Code"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_USER_NAME, harness_name};

    /// Both Copilot client strings map to one name, so a store written
    /// across a Copilot release reads as one agent.
    #[test]
    fn copilot_keeps_its_name_across_client_strings() {
        assert_eq!(
            harness_name("github-copilot-developer"),
            harness_name("copilot-cli")
        );
        assert_eq!(harness_name("something-else"), None);
    }

    #[test]
    fn the_default_user_name_is_capitalised_for_display() {
        assert_eq!(DEFAULT_USER_NAME, "User");
    }
}
