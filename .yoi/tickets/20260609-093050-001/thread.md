<!-- event: create author: LocalTicketBackend at: 2026-06-09T09:30:50Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: intake at: 2026-06-09T09:55:00Z -->

## Intake summary

既存 Ticket を確認し、同目的の未完了重複は見当たりません。要件は Tool 実行 API に `ToolExecutionContext` を導入し、`call_id` / response-local `batch_id` / `call_index` を Tool 実装へ渡すことです。Worker は従来どおり approved tool calls を並列実行し、resource scheduling policy は持たないこと、old Tool API 互換層を長期維持しないこと、Interceptor / hook に lock lifecycle を持たせないことが binding decision です。実装側には `batch_id` 生成方式や session log への記録有無の裁量があります。関連し得る Ticket として `20260609-085148-001` は response 単位の batching analytics であり、目的が異なるため重複ではありません。リスク/レビュー焦点は public API 変更、Worker tool execution path、Interceptor/hook 意味論、並列実行維持、既存 built-in tools の一括移行です。受け入れ条件と validation が明確なため implementation_ready と判断します。

---

<!-- event: state_changed author: intake at: 2026-06-09T09:55:00Z from: planning to: ready reason: intake_ready field: state -->

## State changed

Intake により要件・非目標・受け入れ条件・レビュー焦点が routing 可能な状態で確認されたため、planning から ready にします。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-09T10:01:00Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---
