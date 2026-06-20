## Resolution

`00001KVJABS1A` を完了しました。

実装内容:
- Profile launch policy が `manifest.scope` を wholesale replacement しないように修正しました。
- 既に解決済みの Profile / workspace override scope に対して、launch-policy default rules を missing rules として append するようにしました。
- `.yoi/override.local.toml` 等で指定された追加 `scope.allow` / `scope.deny` は保持されます。
- Normal launch の workspace root write scope と `.worktree` write deny は維持されます。
- Ticket role launch の default direct scope / delegation defaults は維持されます。
- Final manifest/snapshot と tool-visible scope が同じ final effective scope を見るように維持しました。
- Restore path は existing `resolved_manifest_snapshot` を使う挙動のままで、override 再評価は追加していません。

主な commit:
- `0717aae3 pod: preserve profile override scope`
- `a1386881 merge: profile override scope`

Review:
- r1 は `approve`。
- Reviewer は scope merge semantics、no authority broadening、workspace write / `.worktree` deny preservation、Ticket role defaults、snapshot/tool-visible scope consistency、restore non-goal preservation を確認しました。

最終 validation:
- `cargo fmt --all --check`
- `git diff --check HEAD^1..HEAD`
- `cargo test -p pod entrypoint::tests::`
- `cargo check -p pod`

Known unrelated note:
- Full `cargo test -p pod` は branch 外の既存 prompt-guidance assertion failure で失敗するため final gate にしませんでした。Reviewer はこの failure が `crates/pod/src/entrypoint.rs` の diff に起因しないことを確認済みです。

Nix validation:
- Not run because no dependency/package/source-filter files changed。

Validation log:
- `/run/user/1000/yoi/yoi-orchestrator/bash-output/bash-WNUQvw.log`