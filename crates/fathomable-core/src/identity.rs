// @okf-doc: /decisions/0058-the-user-has-the-last-word.md
//! Who is who on a thread (ADR 0058): the name an agent signs with and
//! the name the user goes by.
//!
//! An agent is named once, when it subscribes: the persona it gave, else
//! the harness it speaks through, read off the MCP client string with
//! [`harness_name`], else that string itself. [`agent_name`] applies the
//! rule. The user is named by `user { name }` in the config, or
//! [`DEFAULT_USER_NAME`]; the wire keeps `user` as the author, and every
//! rendering substitutes the configured name.
//!
//! # Examples
//!
//! ```
//! use fathomable_core::identity::{agent_name, harness_name};
//!
//! assert_eq!(harness_name("copilot-cli"), Some("Copilot"));
//! assert_eq!(agent_name(None, Some("claude-code")), "Claude");
//! assert_eq!(agent_name(Some("rev"), Some("claude-code")), "rev");
//! assert_eq!(agent_name(None, Some("my-client")), "my-client");
//! assert_eq!(agent_name(None, None), "agent");
//! ```

/// The user's name when the config sets none.
pub const DEFAULT_USER_NAME: &str = "User";

/// The name an agent signs with when neither a persona nor a client is
/// known.
pub const UNKNOWN_AGENT_NAME: &str = "agent";

/// The harness name for the MCP client string `client`, when it is a
/// known one: the string a harness reports changes between its releases
/// (`github-copilot-developer` became `copilot-cli`), the name does not.
#[must_use]
pub fn harness_name(client: &str) -> Option<&'static str> {
    match client {
        "claude-code" => Some("Claude"),
        "copilot-cli" | "github-copilot-developer" => Some("Copilot"),
        "codex-mcp-client" | "codex" => Some("Codex"),
        _ => None,
    }
}

/// The name an agent signs with: `persona` when given, else the harness
/// name for `client`, else `client` itself, else [`UNKNOWN_AGENT_NAME`].
#[must_use]
pub fn agent_name(persona: Option<&str>, client: Option<&str>) -> String {
    persona
        .filter(|name| !name.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| {
            client
                .map(|client| harness_name(client).map_or_else(|| client.to_owned(), str::to_owned))
        })
        .unwrap_or_else(|| UNKNOWN_AGENT_NAME.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_USER_NAME, agent_name, harness_name};

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

    /// A blank persona is no persona: the harness name wins.
    #[test]
    fn a_blank_persona_falls_back_to_the_harness() {
        assert_eq!(agent_name(Some("  "), Some("claude-code")), "Claude");
        assert_eq!(agent_name(Some(""), None), "agent");
    }

    #[test]
    fn the_default_user_name_is_capitalised_for_display() {
        assert_eq!(DEFAULT_USER_NAME, "User");
    }
}
