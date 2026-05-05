# Review: Agent Skills を Workflow として ingest

## 前提・要件の確認

### 完了条件マッピング

| 条件 | 状態 | 根拠 |
|---|---|---|
| `$user/skills/` 配下の `SKILL.md` が Workflow として登録され `/<name>` で呼び出せる | 満 | `crates/pod/src/pod.rs:2849-2862` で `manifest::paths::user_skills_dir()` を `load_skills_from_dir` → `merge_skill`。生成 `WorkflowRecord` は `user_invocable: true`/`model_invokation: true` (`crates/memory/src/skill.rs:73-84`) なので既存 resolver と `list_user_invocable` がそのまま拾う |
| manifest `[skills] directories` 指定 workspace で追加 ingest、未指定なら 0 件 | 満 | `crates/pod/src/pod.rs:2840-2847` が `manifest.skills.as_ref()` 配下のみ走査。未指定なら `if let Some(...)` を抜けるだけで何も読まない |
| 内製 Workflow と同 slug の skill は内製優先で shadow し Notification を発行 | 満 | `WorkflowRegistry::merge_skill` (`crates/memory/src/workflow.rs:146-158`) が既存 record 保持、`ShadowedSkill` を返す。`drain_skill_shadows` (`crates/pod/src/pod.rs:2868-2876`) で `push_notify` |
| skill ディレクトリ全体 (`SKILL.md`/`scripts/`/`references/`/`assets/`) が scope readable | 満 | `skill_dir_read_rules` (`crates/pod/src/pod.rs:2902-2922`) が `Permission::Read` + `recursive: true` で `build_scope_with_memory` の `scope_config.allow.extend` に乗る |
| frontmatter 違反は warn でスキップ、他 skill / Pod 起動に影響しない | 満 | `load_skills_from_dir` (`crates/memory/src/skill.rs:202-248`) が `parse_skill_md` のエラーを `tracing::warn!` で握り潰し、結果を Vec に積む。Pod 構築フローは `Result` を返さない |
| 単体テスト: frontmatter 検証 / Workflow マッピング / 衝突解決 / manifest 未指定時 workspace skip | 概ね満 (1 点 nit) | 後述「テストカバレッジ」参照 |

### 方針マッピング

- ロードソースの優先順 (内製 → workspace → user) — `ingest_skills` (`crates/pod/src/pod.rs:2836-2867`) で実現。registry にあらかじめ載った内製 Workflow が `merge_skill` で常に勝ち、workspace を user より先に feed することで spec 通りの順位が出る
- `model_invokation: true` / `user_invocable: true` 固定、`requires` 空 — `SkillRecord::into_workflow_record` (`crates/memory/src/skill.rs:73-84`) で固定
- `allowed-tools` は warn で受け流し — `crates/memory/src/skill.rs:156-161` で `tracing::warn!`。Permission 層の受け皿契約は `tickets/permission-extension-point.md` 側に追記済み
- 検証は内製 Workflow より緩く個別 skill 単位 skip — `load_skills_from_dir` の per-entry `match` で `tracing::warn!` のみ。Pod 構築は止まらない

## アーキテクチャ・スコープ

### 既存パターンとの整合

- **manifest/scope 拡張**: `SkillsConfig` の `merge` は他の `merge_option` と同じ pattern (`crates/manifest/src/config.rs:215-220`)。`resolve_paths` は既存の `auth.path` などと同様 `join_if_relative(base, dir)` を使い、`TryFrom<PodManifestConfig>` で `ensure_absolute` を呼ぶ三段構え。ScopeConfig の他 field と同じ流儀で破綻なし
- **memory crate**: `skill.rs` を新ファイルに切り出し、`workflow.rs` には `WorkflowSource` / `ShadowedSkill` / `merge_skill` のみを足す。`WorkflowRecord` 自身に skill 起源固有の field を生やしていないので、`load_workflows` 側のエラー型 (`WorkflowLoadError`) はそのまま。SKILL 用エラー型は `SkillParseError` に分離 — 内製 Workflow と外部 skill で「壊れたら hard error / 壊れたら warn skip」の温度差が型レベルで表現されている
- **scope union pattern**: `build_scope_with_memory` が `deny_write_rules`/`skill_dir_read_rules` を `extend` する構造。`scope::deny_write_rules` の参照モデルと完全に対称
- **`paths::user_skills_dir`**: `user_prompts_dir` (`crates/manifest/src/paths.rs:78-81`) と並列に並ぶ命名・実装。違和感なし
- **Cargo**: `serde_yaml` / `tempfile` / `tracing` / `thiserror` は既に memory の依存にあり (`crates/memory/Cargo.toml:16-22`)。新規追加は無く、Cargo 系のガイドライン (cargo add 縛り) には抵触しない

### 範囲外項目への侵入チェック

- `allowed-tools` 実効化: 行わず warn 止まり。`tickets/permission-extension-point.md` 側に受け皿契約を追記しているのは妥当な情報の遺し方で、本チケットのスコープ外実装には踏み込んでいない
- `scripts/` 自動実行: しない。Read 経由の閲覧のみで、Bash 経由実行は Permission 層待ち
- ビルトイン skill / 自動生成 / 間依存: いずれも実装無し
- skill 用 protocol message / event 種別の追加: 無し。`Method::Notify` の既存ルートを再利用

### 不必要な抽象 / 過剰な API のチェック

- `WorkflowSource::label()` は `ShadowedSkill::message()` でしか使われない (`crates/memory/src/workflow.rs:42-49`)。public で晒されているが UI/log の他の経路で再利用される含みはあるので許容範囲
- `SkillRecord` を一旦経由してから `into_workflow_record` で `WorkflowRecord` に投影する 2 段構造は、当面 `SkillRecord` の独自情報 (allowed_tools 等) を持たせる場が無いので冗長気味。ただし「Permission 層が来たときに skill 固有 metadata の保持先になる」という拡張ポイントとして見れば、無駄ではない (受け皿チケット側の文面とも整合)
- `WorkflowRegistry::merge_skill` の「呼び出し側に降順 priority feed を強制し registry は再ランクしない」契約は doc comment で明記されており、ingest 側 `ingest_skills` でその順序を遵守。registry 側で抽象化を増やすより素直

## 指摘事項

### Blocking

なし。

### Non-blocking / Follow-up

- **`restore_from_manifest` でも shadow 通知を emit する設計の説明不足** — `crates/pod/src/pod.rs:2501` で resume 時にも `drain_skill_shadows` を呼ぶ。実装ロジックとしては「registry を再構築する以上、衝突は再評価される」という意味で一貫しているが、ユーザーから見ると同じ session が一度落ちて起動するたびに `[Skill shadowed]` が再注入される。`drain_skill_shadows` か `restore_from_manifest` 周辺に「resume 時にも (重複で) 出すのは仕様」と一行コメントを残しておくと、後で疑問が再燃しない
- **`WorkflowRegistry::merge_skill` の事前条件依存性** — 「降順 priority feed を呼び出し側が守る」契約は doc comment にあるが、debug assertion (例えば 2 回目以降の skill record が既存 skill record を shadow しようとしたとき、`ws/user` の順序が逆だったら `debug_assert!`) は無い。将来 `ingest_skills` 以外の caller が増えたときに気付きにくいので、follow-up で防衛コードを足す余地あり
- **`Slug::parse` が agent-skills 仕様より厳しい** — 仕様は `[a-z0-9-]+` だが `Slug` は先頭末尾の `-` と `--` を禁止。今 `[a-z0-9-]+` 厳密準拠の skill は実用上ほぼ無く、内製 Workflow の slug ルールと一致させる方が合理的なので、現状の厳しめ運用で良い。ただしユーザー観点での挙動 (warn で skip される) を docs/plan で明示しておく価値はある
- **scope.allow が常に user_skills_dir を含む** — `skill_dir_read_rules` (`crates/pod/src/pod.rs:2902-2922`) は manifest 側に skills 設定が無くとも `~/.config/insomnia/skills` を allow に追加する。`resolve_path` は存在しない path を最寄りの祖先に anchor して許容するので技術的には壊れないが、「ユーザーがそのディレクトリを意図せず作っただけで Pod の scope に readable として乗る」という挙動は ticket の背景 (デフォルトで有効) に沿っており妥当。ただし「ユーザーが skills 機能を完全に opt-out したい」場合の手段が現状無いので、Phase 2 で `[skills] enabled = false` 等の opt-out を検討する余地あり
- **`pod.rs` のテスト 3 件が `skill_dir_read_rules`/`ingest_skills` のホワイトボックステスト** — 衝突発生時の `shadow` Notify 経路 (`drain_skill_shadows` が `push_notify` を確実に呼ぶこと) を end-to-end で検証するテストはまだ無い。`memory/src/workflow.rs` 側で `merge_skill` の戻り値検証はあるので致命的ではないが、Pod 構築から Notification 到達までの統合的な確認は将来 follow-up に回せる

### Nits

- `crates/memory/src/skill.rs:316-326` (`invalid_slug_name_is_error`): `BAD-Caps` という入力は dir 名としても禁止されている (大文字を含むため `Slug::parse` が rejected する)。コメントで「`InvalidName` か `NameDirMismatch` のどちらかになる」と書いてあるが、実際にどちらが先に発火するかをテストで明示しておくと、validation 順序を変えたときに気付きやすい
- `crates/pod/src/pod.rs:14-16`: `Permission` を import 行に追加した整形で行が 100 文字を超えそうに見える (rustfmt は通っているはずなので問題ないが、`use manifest::{}` ブロックがリフォーマットしたほうが読みやすい)
- `[Skill shadowed]` 文言は agent 側に渡る system message としては短くて十分だが、今後類似のシステム由来 notify が増えたときに prefix の語彙統一 (`[Memory shadowed]` / `[Knowledge ...]` 等) を docs/plan で揃える価値あり

## テストカバレッジ

ticket の完了条件にある「frontmatter 検証、Workflow へのマッピング、衝突解決、manifest 未指定時の workspace skip」を順に確認:

- frontmatter 検証: `parses_minimal_skill` / `name_dir_mismatch_is_error` / `invalid_slug_name_is_error` / `empty_description_is_error` / `description_at_cap_is_accepted` / `description_over_cap_is_error` / `missing_frontmatter_is_error` / `extra_frontmatter_fields_are_kept` (`crates/memory/src/skill.rs`) — 過不足なし
- Workflow マッピング: `into_workflow_record_uses_skill_defaults` (`crates/memory/src/skill.rs:415-435`) で `model_invokation`/`user_invocable`/`requires`/`source` を確認
- 衝突解決:
  - `merge_skill_inserts_when_no_collision` / `merge_skill_shadows_existing_workflow` / `merge_skill_priority_workspace_over_user` / `shadow_message_is_human_readable` (`crates/memory/src/workflow.rs:396-490`)
- manifest 未指定時の workspace skip: 直接の単体テストは無いが、`ingest_skills_loads_from_workspace_directories` (`crates/pod/src/pod.rs:3068-3087`) で `manifest.skills = Some(...)` のケースを cover、`skill_dir_read_rules_ignores_missing_skills_section` (`crates/pod/src/pod.rs:3057-3066`) が Skill 設定無しでの allow rules を検査。`ingest_skills` の構造的に未指定なら for ループに入らない事は自明だが、念のため「未指定 manifest で `ingest_skills` が空 Vec を返す」テストを 1 件足すとカバレッジが完結する (non-blocking)

`cargo test --workspace --no-fail-fast` を実行: 全 crate で 0 failed を確認。

## 判断

**Approve** — チケットの完了条件は要件・テストカバレッジともすべて満たされており、既存の manifest cascade / scope union / push_notify / WorkflowRegistry の各パターンに沿って実装されている。範囲外項目 (`allowed-tools` 実効化、`scripts/` 自動実行、ビルトイン skill 等) には踏み込まず、受け皿契約の追記も適切。Non-blocking の follow-up は将来チケットで巻き取って良い性質のもの。
