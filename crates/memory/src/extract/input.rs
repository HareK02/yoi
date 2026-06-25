//! extract sub-Engine への入力テキスト組み立て。
//!
//! `crates/pod/src/pod.rs::build_summary_prompt` と同じ方針で
//! Item 列を flat な行に落とす（reasoning は省く、tool call は名前のみ、
//! tool result は summary のみ）。conversation 全体を Markdown の単一
//! セクションとして渡し、抽出指示は system prompt 側に寄せる。

use llm_engine::Item;

/// 与えられた `items` を extract sub-Engine の最初の user 入力に整形する。
pub fn build_extract_input(items: &[Item]) -> String {
    let mut out = String::new();
    out.push_str(
        "Extract activity logs from the conversation slice below. \
         Follow the system prompt's schema strictly and call `write_extracted` once.\n\n",
    );
    out.push_str("## Conversation slice\n");
    out.push_str(&render_items(items));
    out.push_str("\n\nWhen you are done, call `write_extracted` and end the turn.");
    out
}

fn render_items(items: &[Item]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for item in items {
        match item {
            Item::Message { role, content, .. } => {
                let role_label = match role {
                    llm_engine::Role::User => "User",
                    llm_engine::Role::Assistant => "Assistant",
                    llm_engine::Role::System => "System",
                };
                let text: String = content
                    .iter()
                    .map(|p| p.as_text())
                    .collect::<Vec<_>>()
                    .join("");
                lines.push(format!("[{role_label}] {text}"));
            }
            Item::ToolCall { name, .. } => {
                lines.push(format!("[ToolCall] {name}"));
            }
            Item::ToolResult { summary, .. } => {
                lines.push(format!("[ToolResult] {summary}"));
            }
            Item::Reasoning { .. } => {}
        }
    }
    lines.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_user_assistant_pair_and_tool_calls() {
        let items = vec![
            Item::user_message("hello"),
            Item::assistant_message("hi"),
            Item::tool_call("c1", "read_file", "{}"),
            Item::tool_result("c1", "ok"),
            Item::reasoning("internal scratch — should be skipped"),
        ];
        let s = build_extract_input(&items);
        assert!(s.contains("[User] hello"));
        assert!(s.contains("[Assistant] hi"));
        assert!(s.contains("[ToolCall] read_file"));
        assert!(s.contains("[ToolResult] ok"));
        assert!(!s.contains("scratch"));
    }

    #[test]
    fn tool_result_renders_summary_but_not_content() {
        let huge_content = "raw-content-should-never-enter-extract-input".repeat(10_000);
        let items = vec![Item::tool_result_with_content(
            "c1",
            "short summary kept for extraction",
            huge_content.clone(),
        )];

        let s = build_extract_input(&items);
        assert!(s.contains("[ToolResult] short summary kept for extraction"));
        assert!(!s.contains("raw-content-should-never-enter-extract-input"));
        assert!(!s.contains(&huge_content));
    }
}
