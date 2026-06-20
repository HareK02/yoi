## Resolution

`00001KVJA7V2R` を完了しました。

実装内容:
- `WebFetch` に `application/pdf` handling を追加しました。
- PDF bytes は UTF-8 / `reject_binary()` text path を bypass します。
- `pdf_extract::extract_text_from_mem_by_pages()` を `tokio::task::spawn_blocking` 内で使用します。
- PDF output は `## Page 1`, `## Page 2` のような page-delimited text として返します。
- `transformed_as` / `pdf_extraction.method` は `pdf_text_by_pages` を使い、semantic Markdown とは主張しません。
- `pdf_extraction` metadata に method/page/readability/diagnostic 情報を追加しました。
- `max_response_bytes` / `max_output_bytes` / redirects / private-local host rejection / embedded credential rejection など既存 WebFetch safety pipeline は維持しました。
- `application/pdf` のみ対応し、extension sniffing や `application/octet-stream` PDF guessing は追加していません。
- Unsupported binary MIME rejection は維持しました。
- Existing HTML/text behavior and `html_extraction` metadata は維持しました。
- Tests for valid page-delimited PDF output、PDF truncation、malformed PDF diagnostic error、unsupported binary rejection を追加しました。
- `pdf-extract = "0.10.0"` dependency を追加し、`Cargo.lock` / `package.nix` `cargoHash` を更新しました。

主な commit:
- `b1af95ad web: fetch pdf text by pages`
- `97edfe8a merge: webfetch pdf text`

Review:
- r1 は `approve`。
- Reviewer は WebFetch safety pipeline、exact `application/pdf` handling、binary path separation、`pdf_text_by_pages` metadata、output bounds、unsupported binary rejection、HTML metadata preservation、native PDF runtime dependency が無いことを確認しました。

最終 validation:
- `cargo fmt --check`
- `git diff --check HEAD^1..HEAD`
- `cargo test -p tools web`
- `cargo check -p tools`
- `cargo tree -p pdf-extract`
- `nix build .#yoi --no-link`

Package impact:
- New Rust dependency: `pdf-extract 0.10.0`
- `nix path-info -S .#yoi`: `115259736`

Validation log:
- `/run/user/1000/yoi/yoi-orchestrator/bash-output/bash-z7rcEU.log`