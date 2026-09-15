// @okf-doc: /decisions/0080-automatic-chat-identity.md
//! Harness-qualified chat identities shared by MCP calls and delivery hooks.

use clap::ValueEnum;

/// The harness that owns a chat, not the model or agent role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Harness {
    Claude,
    Copilot,
    Codex,
    Vscode,
}

impl Harness {
    pub(crate) fn from_client(client: &str) -> Option<Self> {
        match fathomable_core::identity::harness_name(client) {
            Some("Claude") => Some(Self::Claude),
            Some("Copilot") => Some(Self::Copilot),
            Some("Codex") => Some(Self::Codex),
            Some("VS Code") => Some(Self::Vscode),
            _ => None,
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Copilot => "Copilot",
            Self::Codex => "Codex",
            Self::Vscode => "VS Code",
        }
    }

    pub(crate) fn key(self, session: &str) -> Result<String, String> {
        if session.trim().is_empty() {
            return Err("the harness supplied an empty chat identity".to_owned());
        }
        let prefix = match self {
            Self::Claude => "claude",
            Self::Copilot => "copilot",
            Self::Codex => "codex",
            Self::Vscode => "vscode",
        };
        Ok(format!("{prefix}:{session}"))
    }
}

/// The native session id a wake command passes back to the harness.
pub(crate) fn session(key: &str) -> Result<&str, String> {
    match key.split_once(':') {
        Some(("claude" | "copilot" | "codex" | "vscode", id)) if !id.trim().is_empty() => Ok(id),
        _ => Err(format!("invalid harness-qualified chat identity: {key}")),
    }
}
