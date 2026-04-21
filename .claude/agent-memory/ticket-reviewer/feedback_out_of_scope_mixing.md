---
name: Out-of-scope diff mixed into ticket
description: 本チケットのスコープ外修正が同じ diff/作業ツリーに同居していた場合のレビュー判定ルール
type: feedback
---

ticket の実装 diff にスコープ外の修正（別の疎通バグ fix、別レイヤの API 調整等）が同居している状況では、**major 扱いの Non-blocking**（= Approve 可、ただし follow-up 指摘）で扱うのが precedent。

**Why:** ユーザー自身が「別コミット候補」と認識した上で差分提示してくるケースが複数回ある。コミット分割はユーザーの git 操作領域（CLAUDE.md: Git はユーザー責務）なので、reviewer 側は**コミット分割を推奨する**指摘に留める。blocker にはしない。

**How to apply:**
- review.md の Non-blocking セクションで「スコープ外 diff が同居している」項目を [major] で立て、該当ファイルと何が本筋外かを列挙する。
- 「本チケットの review は X 単体の妥当性判定に留め、スコープ外修正の可否まで巻き込まない」ことを明記。
- 本筋（チケット要件）が満たされていれば総合判定は Approve で良い。
- 挙動保存の確認は本筋と同時に行うが、スコープ外変更の影響で疎通確認が混同しないか（どの変更が疎通パスした根拠か）を review.md に一言書く。

precedent: `tickets/llm-capability-ownership.review.md` (2026-04-21)
