// @okf-doc: /decisions/0080-automatic-chat-identity.md
//! Harness-qualified chat identities for MCP annotation authors.

/// The harness that owns a chat, not the model or agent role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
