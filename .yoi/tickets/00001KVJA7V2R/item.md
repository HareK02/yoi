---
title: 'WebFetch: PDF を page-delimited text として取得できるようにする'
state: 'closed'
created_at: '2026-06-20T10:46:48Z'
updated_at: '2026-06-20T12:31:33Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['security', 'dependency', 'public-api', 'output-bounds']
queued_by: 'workspace-panel'
queued_at: '2026-06-20T12:06:29Z'
---

## Background

ユーザー要望: `WebFetch` で PDF URL を取得し、LLM が読める bounded text として返せるようにする。調査は同一会話内で完了済みで、初期実装は semantic な「PDF to Markdown」ではなく、PDF から page-delimited な Markdown-ish text を抽出する方針とする。

現状:

- `crates/tools/src/web.rs` の `WebFetch` は HTML / text / JSON / XML-ish content のみを許可し、`application/pdf` は unsupported Content-Type として拒否する。
- 既存の `render_content()` は `reject_binary(bytes)` と UTF-8 decode 前提なので、PDF binary を既存 text path に乗せることはできない。
- `WebFetch` の既存 safety behavior は維持する必要がある: private/local host rejection、bounded redirects、`max_response_bytes`、`max_output_bytes`、untrusted content warning。

調査結論:

- 初期実装には `pdf-extract` crate が最有力。
  - MIT license。
  - pure Rust。
  - native/system dependency なし。
  - memory buffer API あり。
  - page split API `pdf_extract::extract_text_from_mem_by_pages()` あり。
- Poppler / `pdftotext` は GPL / system dependency / deployment / Nix packaging の重さから採用しない。
- `pdfium-render` は Pdfium native library が必要で、初期の text extraction には重すぎるため採用しない。
- `unpdf` / `pdf_oxide` / `spectre_pdf` 等は Markdown-oriented な候補だが、初期採用には成熟度・audit が不足気味。必要なら follow-up で比較する。

## Requirements

- `WebFetch` が `application/pdf` response を unsupported Content-Type として拒否せず処理できる。
- PDF binary bytes は既存の UTF-8 text path / `reject_binary()` path と分離して扱う。
- `pdf_extract::extract_text_from_mem_by_pages()` を使い、page ごとに抽出した text を Markdown-ish に整形する。
- 出力例:

```markdown
## Page 1

...

## Page 2

...
```

- `transformed_as` は `pdf_text_by_pages` など、semantic Markdown 化を約束しない名前にする。
- result JSON に PDF 用 metadata を追加する。
- text/html / JSON / XML / text の既存挙動と `html_extraction` metadata を regress させない。
- `WebFetch` の既存 bounds と safety behavior を維持する。

## Acceptance criteria

- `application/pdf` response が bounded text result を返す。
- PDF response では `render_content()` の UTF-8 decode 前提 path を通らず、PDF 専用 extraction path が使われる。
- `pdf_extract::extract_text_from_mem_by_pages()` により、page delimiter 付き Markdown-ish text が返る。
- result JSON に `pdf_extraction` metadata が含まれる。
- `max_output_bytes` を超える抽出結果は既存 truncation marker で切り詰められ、`output_truncated` が正しく立つ。
- `max_response_bytes` / Content-Length check / redirect limit / private host rejection / embedded credential rejection は既存通り維持される。
- malformed PDF / encrypted PDF / text のない PDF で panic しない。diagnostic error または readable=false 相当の metadata を返す。
- PDF 以外の unsupported binary content は引き続き拒否される。
- 既存 WebFetch HTML reader tests が通る。

## Binding decisions / invariants

- `WebFetch` は fetch/extraction tool のままとし、LLM summarization や multi-page research orchestration は入れない。
- 初期実装では semantic な「PDF to Markdown」を約束しない。page-delimited Markdown-ish text extraction として扱う。
- 初期実装で対応する MIME は `application/pdf` のみとする。`application/x-pdf`、`application/acrobat`、`application/octet-stream` with `.pdf`、extension sniffing は必要なら follow-up。
- `max_response_bytes` default は変更しない。大きい PDF が必要な場合は既存 config override を使う。
- Poppler / Pdfium / subprocess `pdftotext` / system native dependency は導入しない。
- OCR、scanned PDF 対応、画像抽出、rendering、table reconstruction、2-column layout の完全復元、heading inference、PDF 保存/cache は範囲外。
- PDF 抽出結果も untrusted content として扱い、既存の warning / network safety / bounds を弱めない。

## Implementation latitude

- PDF metadata は互換性重視で `pdf_extraction` を新設するのが推奨。既存 `html_extraction` は残す。
- `pdf_extraction` metadata の具体フィールドは実装時に調整してよいが、少なくとも method / pages or pages_included / readable / error diagnostic 相当が分かるようにする。
- PDF extraction は CPU-heavy になり得るため、可能なら `spawn_blocking` 等で async runtime を塞がない設計にする。ただし Rust parser の強制 cancel までは初期実装の必須条件にしない。
- 抽出品質が `pdf-extract` で明確に不足する場合は、実装を歪めず、別 Ticket で `unpdf` / `pdf_oxide` 等を比較する。

## Readiness

- readiness: implementation_ready
- risk_flags: [security, dependency, public-api, output-bounds]

## Escalation conditions

- `pdf-extract` が実装上致命的に使えない、または license/build/security 上の問題が見つかった場合は実装前に戻す。
- Tool result JSON shape に breaking change が必要になりそうな場合は、既存互換を維持する案を先に提示する。
- PDF fixture で parser panic / runaway CPU / excessive memory の懸念が出た場合は、bounds 追加または別方針を提案する。
- Nix packaging / Cargo.lock / cargoHash 影響が大きい場合は実装報告で明示する。

## Validation

- focused tests:
  - small valid PDF fixture returns expected text。
  - multi-page PDF fixture returns `## Page 1` / `## Page 2`。
  - output truncation sets `output_truncated`。
  - unsupported binary non-PDF remains rejected。
  - oversized PDF Content-Length remains rejected。
  - existing WebFetch HTML reader behavior remains unchanged。
- commands:
  - `cargo fmt --check`
  - `cargo test -p tools web`
  - `cargo check -p tools`
  - `git diff --check`
  - `TicketDoctor` if Ticket consistency needs checking。

## Related work

- `crates/tools/src/web.rs` — current WebSearch/WebFetch implementation。
- Closed prior WebFetch HTML work: `00001KSX9W968`, `00001KSXECDG0`。
