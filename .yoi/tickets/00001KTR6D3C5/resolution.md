完了しました。

実施内容:
- Lua Profile evaluator に global `yoi` を注入しました。
- `yoi.profile` / `require("yoi.profile")` は callable table になり、既存の `local profile = require("yoi.profile"); return profile { ... }` 互換を保ちながら `import` / `extend` を提供します。
- `yoi.profile.import("builtin:default")` は resolved Manifest ではなく raw builtin Profile artifact を返します。
- `yoi.profile.extend("builtin:default", overrides)` は object を再帰 merge し、scalar/list など non-object は置換します。
- 最終 artifact は既存 Profile validation に通され、runtime-bound field（例: `pod`）の混入は reject されます。
- `resources/profiles/default.lua` を global `yoi` style に更新しました。

Merge:
- Branch: `lua-profile-yoi-api`
- Merge commit: `15dc176e merge: lua profile yoi api`

確認:
- Branch-local reviewer `reviewer-lua-profile-yoi-api` が approve。
- `cargo fmt --check` passed。
- `git diff --check` passed。
- `cargo test -p manifest profile --lib` passed（22 passed）。
- `cargo check -p manifest` passed。
- `target/debug/yoi ticket doctor` passed。
- typed `TicketDoctor` は 0 errors / 3 pre-existing diagnostics。
- `nix build .#yoi` passed。

残作業:
- なし。import/extend は現時点では `builtin:default` / `default` に限定しています。より広い user/project selector import が必要なら follow-up Ticket として扱えます。