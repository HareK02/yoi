完了しました。

実施内容:
- `Tracker` に `Write` / `Edit` 共有の per-target-file mutation guard を追加しました。
- `Write` と `Edit` は read/verify/write/record の critical section を同一ファイル単位で直列化します。
- Worker/provider の parallel tool execution は維持し、Worker-wide scheduler は導入していません。
- guard key は canonical target / canonical parent / lexical fallback で同一ファイル相当を寄せ、異なるファイルは別 mutex で扱います。
- `ToolExecutionContext` の `batch_id` / `call_index` は guard acquisition の diagnostics/correlation に使っています。
- same-file write→edit ordering、failed mutation release、equivalent path guard、different-file non-blocking の tests を追加しました。

Merge:
- Branch: `serialize-file-mutations`
- Merge commit: `29960c15 merge: serialize file mutations`

確認:
- Branch-local reviewer `reviewer-serialize-file-mutations` が approve。
- `cargo fmt --check` passed。
- `git diff --check` passed。
- `cargo test -p tools --lib` passed（99 passed）。
- `cargo check -p tools` passed。
- `target/debug/yoi ticket doctor` passed。
- typed `TicketDoctor` は 0 errors / 3 pre-existing diagnostics。
- `nix build .#yoi` passed。

残作業:
- なし。将来 `Write` / `Edit` が guard acquisition 前に await を増やす場合は、同一 response 内の call order 保証を再確認してください。