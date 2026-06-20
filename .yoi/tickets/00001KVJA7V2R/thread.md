<!-- event: create author: ticket-intake at: 2026-06-20T10:46:48Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-20T10:46:54Z -->

## Intake summary

ユーザー要望を調査 Ticket ではなく concrete implementation Ticket として作成した。調査済み結論に基づき、`WebFetch` が `application/pdf` を `pdf-extract` で page-delimited Markdown-ish text として返せるようにする。Poppler/Pdfium/subprocess/OCR/semantic Markdown 化は非ゴール。既存 WebFetch safety bounds と HTML/text behavior は維持する。

---

<!-- event: state_changed author: ticket-intake at: 2026-06-20T10:46:54Z from: planning to: ready reason: implementation_ready field: state -->

## State changed

Intake 済み。Orchestrator は implementation routing として扱える。実装 side effect / worktree 作成 / coder 起動はここでは行っていない。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-20T12:06:29Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-20T12:08:15Z -->

## Decision

Routing decision: implementation_ready_parallel

Reason:
- Panel Queue により、この Ticket は Orchestrator routing 対象として明示許可された。
- User standing directive: blocker が無いものは並列実行する。現在の `00001KVJABS1A` は Profile scope review 中であり、WebFetch PDF 実装とは domain/file conflict がないため並列化できる。
- Ticket body は調査済みの PDF extraction 方針、`pdf-extract` 採用理由、binary path 分離、page-delimited Markdown-ish output、metadata、bounds、non-goals、validation を実装可能な粒度で定義している。
- 未解決 relation blocker はない。
- Orchestrator worktree は clean、matching branch/worktree はなし。
- Risk domain は security / dependency / public-api / output-bounds だが、Ticket は existing WebFetch network safety、`max_response_bytes` / `max_output_bytes`、unsupported binary rejection、no OCR/semantic Markdown/native dependency を明示している。bounded context check 後も implementation 前に必要な追加 human decision は見つからなかった。

Evidence checked:
- Ticket `00001KVJA7V2R` body / thread / relations / artifacts。
- `TicketRelationQuery(00001KVJA7V2R)`: no blockers。
- `TicketOrchestrationPlanQuery(00001KVJA7V2R)`: no previous plan records; accepted plan recorded now。
- Workspace state:
  - Orchestrator worktree clean at `36b9ed45`。
  - queued: `00001KVJA7V2R`, `00001KVJDJD02`。
  - inprogress: `00001KVJABS1A` review only。
  - no matching WebFetch PDF branch/worktree。

IntentPacket:

Intent:
- Extend `WebFetch` so `application/pdf` can be fetched and returned as bounded, page-delimited text suitable for LLM reading。
- Use `pdf_extract::extract_text_from_mem_by_pages()` and present output as Markdown-ish page sections, not semantic PDF-to-Markdown。

Binding decisions / invariants:
- Keep WebFetch as fetch/extraction tool; no summarization or research orchestration。
- PDF bytes must not go through the UTF-8 text / `reject_binary()` path。
- Preserve private/local host rejection, bounded redirects, Content-Length / `max_response_bytes`, `max_output_bytes`, embedded credential rejection, untrusted content warning。
- Initial supported MIME is `application/pdf` only; no extension sniffing or `application/octet-stream` PDF guessing。
- No Poppler/Pdfium/subprocess/native dependency/OCR/scanned-PDF support/table reconstruction/cache。
- Existing HTML/text/JSON/XML behavior and `html_extraction` metadata must not regress。
- New metadata should use `pdf_extraction` with method/pages/readable/diagnostic information。

Requirements / acceptance criteria:
- Valid PDF returns `## Page 1`, `## Page 2`, etc. page-delimited bounded text。
- `transformed_as` communicates `pdf_text_by_pages` or equivalent, not semantic Markdown。
- `output_truncated` is correct when PDF text exceeds `max_output_bytes`。
- Malformed/encrypted/textless PDFs do not panic; return diagnostic error or readable=false metadata。
- Non-PDF unsupported binary remains rejected。
- Focused WebFetch tests and relevant cargo/fmt/diff validation pass。

Next action:
- Record `queued -> inprogress` and commit Ticket records before creating implementation worktree and spawning Coder。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-20T12:08:34Z from: queued to: inprogress reason: orchestrator_acceptance_parallel_webfetch_pdf field: state -->

## State changed

Ticket body/thread, relation metadata, Orchestrator worktree state, visible Pods, existing branch/worktree, and bounded WebFetch/PDF context were checked. There is no unresolved blocking dependency, no conflicting inprogress implementation, and no missing planning decision. Accepting this queued Ticket for parallel implementation before worktree/Pod side effects.

---
