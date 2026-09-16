// @okf-doc: /decisions/0080-automatic-chat-identity.md
//! Resolve each request's chat identity without model arguments or connection memory.

use std::env;

use rmcp::RoleServer;
use rmcp::service::RequestContext;

use crate::caller::Harness;

/// Launch identities apply only to the harness that supplied them.
#[derive(Debug, Default)]
pub(super) struct Launch {
    copilot: Option<String>,
    claude: Option<String>,
}

/// A chat whose identity came from its harness.
#[derive(Debug, Clone)]
pub(super) struct Caller {
    pub(super) id: String,
    pub(super) harness: Harness,
    pub(super) client: String,
}

impl Launch {
    pub(super) fn from_env() -> Self {
        Self {
            copilot: environment("COPILOT_AGENT_SESSION_ID"),
            claude: environment("CLAUDE_CODE_SESSION_ID"),
        }
    }

    pub(super) fn resolve(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> Result<Option<Caller>, String> {
        let Some(client) = context.client_info().map(|client| client.name) else {
            return Ok(None);
        };
        let Some(harness) = Harness::from_client(&client) else {
            return Ok(None);
        };
        let id = match harness {
            Harness::Copilot => self.copilot.as_deref(),
            Harness::Claude => self.claude.as_deref(),
            Harness::Vscode => metadata(context, "vscode.conversationId")?,
            Harness::Codex => {
                // sessionId is the family shared with descendants; threadId
                // identifies the concrete resumable chat and its hook session.
                let family = metadata(context, "sessionId")?;
                let thread = metadata(context, "threadId")?;
                family.and(thread)
            }
        };
        id.map(|id| {
            Ok(Caller {
                id: harness.key(id)?,
                harness,
                client,
            })
        })
        .transpose()
    }

    pub(super) fn require(&self, context: &RequestContext<RoleServer>) -> Result<Caller, String> {
        self.resolve(context)?.ok_or_else(|| {
            let client = context.client_info().map(|client| client.name);
            let expected = match client.as_deref().and_then(Harness::from_client) {
                Some(Harness::Copilot) => "COPILOT_AGENT_SESSION_ID in the MCP launch environment",
                Some(Harness::Claude) => {
                    "CLAUDE_CODE_SESSION_ID from a fresh Claude invocation or explicit --resume ID"
                }
                Some(Harness::Vscode) => "vscode.conversationId in this request's _meta",
                Some(Harness::Codex) => "sessionId and threadId in this request's _meta",
                None => "a supported Copilot, Claude Code, VS Code, or Codex client identity",
            };
            format!(
                "chat identity unavailable: expected {expected}. Reading is available, but \
                 posting requires automatic identity. Do not ask the user \
                 for an internal session id"
            )
        })
    }
}

fn environment(name: &str) -> Option<String> {
    env::var_os(name).and_then(|value| match value.into_string() {
        Ok(id) if !id.trim().is_empty() => Some(id),
        _ => {
            tracing::warn!(
                variable = name,
                "ignoring invalid chat identity: expected nonempty Unicode"
            );
            None
        }
    })
}

fn metadata<'a>(
    context: &'a RequestContext<RoleServer>,
    key: &str,
) -> Result<Option<&'a str>, String> {
    context
        .meta
        .get(key)
        .map(|value| {
            value
                .as_str()
                .filter(|id| !id.trim().is_empty())
                .ok_or_else(|| {
                    format!("invalid chat identity: _meta.{key} must be a nonempty string")
                })
        })
        .transpose()
}
