use async_trait::async_trait;
use llm_engine::llm_client::client::LlmClient;
use manifest::{ToolPermissionAction, ToolPermissionConfig};
use serde_json::Value;
use session_store::Store;

use crate::Pod;
use crate::hook::{Hook, HookPreToolAction, PreToolCall, ToolCallSummary};

/// Built-in manifest permission policy for `PreToolCall`.
///
/// This hook is registered by Pod before user hooks, so manifest-level deny
/// rules fail closed before user extension code can approve a call.
pub(crate) struct PermissionHook {
    config: ToolPermissionConfig,
}

impl PermissionHook {
    pub(crate) fn new(config: ToolPermissionConfig) -> Self {
        Self { config }
    }

    fn action_for(&self, input: &ToolCallSummary) -> ToolPermissionAction {
        let target = permission_target(&input.arguments);
        self.config
            .rules
            .iter()
            .find(|rule| {
                rule.tool.eq_ignore_ascii_case(&input.tool_name)
                    && wildcard_match(&rule.pattern, &target)
            })
            .map(|rule| rule.action)
            .unwrap_or(self.config.default_action)
    }
}

impl<C: LlmClient, St: Store> Pod<C, St> {
    pub(crate) fn apply_permissions_from_manifest(&mut self) {
        let Some(permissions) = self.manifest().permissions.clone() else {
            return;
        };
        self.add_pre_tool_call_hook(PermissionHook::new(permissions));
    }
}

#[async_trait]
impl Hook<PreToolCall> for PermissionHook {
    async fn call(&self, input: &ToolCallSummary) -> HookPreToolAction {
        match self.action_for(input) {
            ToolPermissionAction::Allow => HookPreToolAction::Continue,
            ToolPermissionAction::Deny => HookPreToolAction::Deny(permission_denied_message(input)),
            ToolPermissionAction::Ask => {
                HookPreToolAction::Deny(permission_ask_unsupported_message(input))
            }
        }
    }
}

fn permission_denied_message(input: &ToolCallSummary) -> String {
    format!(
        "permission denied: tool `{}` arguments matched the manifest permission policy",
        input.tool_name
    )
}

fn permission_ask_unsupported_message(input: &ToolCallSummary) -> String {
    format!(
        "permission ask unsupported: tool `{}` requires approval, but this runtime has no permission approval protocol; denied fail-closed",
        input.tool_name
    )
}

fn permission_target(arguments: &Value) -> String {
    if let Value::Object(map) = arguments {
        for key in ["command", "file_path", "path", "pattern", "query", "url"] {
            if let Some(value) = map.get(key).and_then(Value::as_str) {
                return value.to_string();
            }
        }
    }
    serde_json::to_string(arguments).unwrap_or_else(|_| arguments.to_string())
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut star_text = 0usize;

    while ti < text.len() {
        if pi < pattern.len() && (pattern[pi] == b'?' || pattern[pi] == text[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pattern.len() && pattern[pi] == b'*' {
            star = Some(pi);
            pi += 1;
            star_text = ti;
        } else if let Some(star_pi) = star {
            pi = star_pi + 1;
            star_text += 1;
            ti = star_text;
        } else {
            return false;
        }
    }

    while pi < pattern.len() && pattern[pi] == b'*' {
        pi += 1;
    }

    pi == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook::HookPreToolAction;
    use manifest::ToolPermissionRule;

    fn summary(tool_name: &str, arguments: Value) -> ToolCallSummary {
        ToolCallSummary {
            call_id: "call_1".into(),
            tool_name: tool_name.into(),
            arguments,
        }
    }

    #[test]
    fn first_matching_rule_wins_by_declaration_order() {
        let hook = PermissionHook::new(ToolPermissionConfig {
            default_action: ToolPermissionAction::Deny,
            rules: vec![
                ToolPermissionRule {
                    tool: "bash".into(),
                    pattern: "git *".into(),
                    action: ToolPermissionAction::Allow,
                },
                ToolPermissionRule {
                    tool: "Bash".into(),
                    pattern: "git reset *".into(),
                    action: ToolPermissionAction::Deny,
                },
            ],
        });

        let input = summary("Bash", serde_json::json!({ "command": "git reset --hard" }));

        assert_eq!(hook.action_for(&input), ToolPermissionAction::Allow);
    }

    #[test]
    fn default_action_applies_when_no_rule_matches() {
        let hook = PermissionHook::new(ToolPermissionConfig {
            default_action: ToolPermissionAction::Deny,
            rules: Vec::new(),
        });

        let input = summary("Read", serde_json::json!({ "file_path": "/tmp/a.txt" }));

        assert_eq!(hook.action_for(&input), ToolPermissionAction::Deny);
    }

    #[tokio::test]
    async fn deny_and_ask_fail_closed_as_public_deny_actions() {
        let deny = PermissionHook::new(ToolPermissionConfig {
            default_action: ToolPermissionAction::Deny,
            rules: Vec::new(),
        });
        let denied = deny
            .call(&summary(
                "Bash",
                serde_json::json!({ "command": "rm -rf target" }),
            ))
            .await;
        match denied {
            HookPreToolAction::Deny(message) => {
                assert!(message.contains("permission denied"));
                assert!(message.contains("Bash"));
            }
            other => panic!("expected fail-closed deny action, got {other:?}"),
        }

        let ask = PermissionHook::new(ToolPermissionConfig {
            default_action: ToolPermissionAction::Ask,
            rules: Vec::new(),
        });
        let asked = ask
            .call(&summary(
                "Bash",
                serde_json::json!({ "command": "git status" }),
            ))
            .await;
        match asked {
            HookPreToolAction::Deny(message) => {
                assert!(message.contains("permission ask unsupported"));
                assert!(message.contains("denied fail-closed"));
            }
            other => panic!("expected ask fail-closed deny action, got {other:?}"),
        }
    }

    #[test]
    fn target_prefers_known_builtin_argument_fields() {
        assert_eq!(
            permission_target(&serde_json::json!({ "command": "rm -rf target" })),
            "rm -rf target"
        );
        assert_eq!(
            permission_target(&serde_json::json!({ "file_path": "/tmp/.env" })),
            "/tmp/.env"
        );
    }

    #[test]
    fn wildcard_supports_star_and_question() {
        assert!(wildcard_match("rm *", "rm -rf target"));
        assert!(wildcard_match("file?.rs", "file1.rs"));
        assert!(!wildcard_match("rm *", "git status"));
    }
}
