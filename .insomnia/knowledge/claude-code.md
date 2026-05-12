---
  created_at: 2026-05-11T22:10:00Z
  updated_at: 2026-05-11T22:10:00Z
  kind: policy
  description: Claude Codeを用いてレビューやinsomniaだけではできないタスクを行う
  model_invokation: false
  user_invocable: true
  last_sources: []
---

Bashツールを用いて`claude`を呼び出す。

`claude -p "<prompt>"`で非対話モードでのClaude Codeの利用が出来る。

また、`claude -p "<prompt>" --continue`を用いることで、直前のセッションを再開する形で実行できる。


insomniaではまだできないのでclaudeにやらせたいタスク
- WebSearch / WebFetch
-
