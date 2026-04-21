---
name: Test-path omission precedent
description: 要件で列挙されたテストパスを「共通ヘルパなので省略」した場合の判断相場
type: feedback
---

チケット要件にテストパス (例: 成功/失敗/mid-turn の 3 本) が明示列挙されている場合、
そのうち 1 本を「共通ヘルパ経由だから inspection で担保」として省略する実装が来たら、
**Approve with follow-up** が相場。Blocking にはしない。

**Why:** 共通化されたインスツルメント (例: `send_event`) 1 点だけが共通で、
呼び出し側の制御フロー (async 再帰・フラグ管理・エラー伝播) は個別なのが通例。
ただしビルドと主要パスが動いており、後続チケットでテストを足すだけの差分で済むケースが多い。
protocol-design (2026-04-21) で先例。

**How to apply:** 要件とテストコードを 1:1 で突き合わせ、欠けたパスがあれば
- 制御フローが共通化されていれば Follow-up
- 制御フローが別物 (別関数 / 別状態遷移) なら Request changes
と切り分ける。`send_event` 型のヘルパ共通化は Follow-up 側の判断。
