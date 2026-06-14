承認します。

Evidence:
- `crates/manifest/src/profile.rs` の Lua API は `yoi.profile.import(...)` を残し、`yoi.profile.extend(...)` は常にエラーにする互換診断スタブに変更されている。旧 `deep_merge_profile_json` と hidden deep-merge composition は削除されており、診断文も `yoi.profile.import(...)` と explicit Lua assignment への移行を示している。
- Builtin role profiles (`resources/profiles/{companion,intake,orchestrator,coder,reviewer}.lua`) は `yoi.profile.import("builtin:default")` 後に Lua の明示代入で必要フィールドだけを上書きしている。feature policy は各 role で全 key を明示し、worker instruction は table mutation で default reasoning 等を保持している。
- scope/delegation は既存 role の明示値を移植しており、profile composition の変更に留まっている。launch policy / authority semantics は別 Ticket `00001KV11DHGZ` の範囲として残され、今回の diff では拡張されていない。
- Tests cover import + explicit assignment, object replacement instead of deep merge, removed extend diagnostic, and builtin role resolution/policy (`profile::tests`).
- Resource embedding/packaging risk is low: changed files are existing `resources/profiles/*.lua` loaded through the existing include table; `cargo build -p yoi` passed.

Validation performed:
- `git diff f709fc10..HEAD` inspected.
- `cargo fmt --check` passed.
- `git diff --check f709fc10..HEAD` passed.
- `cargo test -p manifest profile::tests:: -- --nocapture` passed.
- `cargo build -p yoi` passed.

Not rerun:
- `nix build .#yoi` was not rerun in this reviewer scope because it would write outside the permitted `target/`/Ticket-record areas unless using broader Nix/store/result-link authority. Coder reported it was run; cargo build validated resource embedding on this branch.

Risks / notes:
- Existing callers of `yoi.profile.extend` now fail intentionally; this is the requested break. The retained stub is a diagnostic compatibility trap, not a merge API.
