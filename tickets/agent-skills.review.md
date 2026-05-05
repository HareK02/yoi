# Review: Agent Skills を Workflow として ingest

## Round 1 (2026-05-04, 旧仕様)

初回レビュー時点では「manifest workspace skills + `$config_dir/skills/` 自動 ingest」の 2 ソース構成を前提に **Approve** とした。`WorkflowSource` も `WorkspaceSkill` / `UserSkill` の 2 バリアントだった。Round 1 の本文は git 履歴で参照可能 (差し戻し履歴の保全)。

その後の設計変更で「skill ロードソースは manifest `[skills] directories` のみ。`$config_dir/skills/` の暗黙ロードと `paths::user_skills_dir()` は廃止」へ移行し、`WorkflowSource` も単一バリアント `Skill { dir }` に統合された。本ファイルは新設計に対する Round 2 のレビューに書き換える。

---

## Round 2 (2026-05-04, 設計変更後)

### 前提・要件の確認 (新仕様の完了条件マッピング)

| 完了条件 | 状態 | 根拠 |
|---|---|---|
| manifest `[skills] directories` で指定したパス配下の SKILL.md が Workflow として登録され `/<name>` で呼べる | 満 | `crates/pod/src/pod.rs:2833-2851` の `ingest_skills` が `manifest.skills.directories` を順に走査し `merge_skill`。生成 `WorkflowRecord` は `model_invokation: true`/`user_invocable: true`/`requires: []` 固定 (`crates/memory/src/skill.rs:73-84`) なので既存 `/<slug>` resolver と `list_user_invocable` がそのまま拾う |
| `[skills]` 未指定時は skill 由来 Workflow 0 件 | 満 | `ingest_skills` (`crates/pod/src/pod.rs:2837-2840`) は `manifest.skills.as_ref()` が `None` なら即 return。`paths::user_skills_dir()` も `crates/manifest/src/paths.rs` から削除済みで暗黙ロード経路自体が存在しない。テスト `ingest_skills_returns_empty_when_skills_section_missing` (`crates/pod/src/pod.rs:3043-3050`) で覆われている |
| 同一 manifest 内の `directories` 列挙順 = 優先順位 (先に書かれたものが上位) | 満 | `merge_skill` は first-insert wins (`crates/memory/src/workflow.rs:142-154`)、`ingest_skills` は `for dir in &skills_cfg.directories` で vec の先頭から feed する。テスト `merge_skill_first_fed_wins_on_collision` (`crates/memory/src/workflow.rs:431-467`) で契約を verify |
| 内製 Workflow 優先で shadow + Notification 発行 | 満 | `merge_skill` は既存 record を保持して `ShadowedSkill` を返す。`drain_skill_shadows` (`crates/pod/src/pod.rs:2855-2862`) が `push_notify` で `[Skill shadowed] ...` を Pod の notify バッファへ。`merge_skill_shadows_existing_workflow` (`crates/memory/src/workflow.rs:401-428`) で workspace workflow > skill の優先関係を verify |
| skill ディレクトリ全体 (`SKILL.md` + `scripts/` + `references/` + `assets/`) が scope readable | 満 | `skill_dir_read_rules` (`crates/pod/src/pod.rs:2888-2901`) が manifest `[skills] directories` の各 entry を `Permission::Read`+`recursive: true` で `scope_config.allow.extend`。manifest 未指定時は空 Vec を返す (`crates/pod/src/pod.rs:2889-2891`) ので「設定無しでも `~/.config/insomnia/skills` を allow に乗せる」Round 1 の懸念は構造的に消滅している |
| frontmatter 違反は warn でスキップ、他 skill / Pod 起動は止まらない | 満 | `load_skills_from_dir` (`crates/memory/src/skill.rs:202-248`) は per-entry `match` で `tracing::warn!`。`Pod::from_manifest` 経路は `Result` を返さない |
| 単体テスト (frontmatter / mapping / 衝突解決 / manifest 未指定) | 満 | 後述「テストカバレッジ」参照 |

### アーキテクチャ・スコープ

#### 既存パターンとの整合
- **manifest cascade**: `SkillsConfig::merge` (`crates/manifest/src/config.rs:218-222`) は `self.directories.extend(upper.directories)` で他の Vec 系設定 (scope rules) と同じ累積規則。`resolve_paths` (`crates/manifest/src/config.rs:189-193`) は他 path フィールドと同じ `join_if_relative(base, dir)`。`TryFrom<PodManifestConfig>` で `ensure_absolute("skills.directories", ...)` を呼ぶ三段構え (`crates/manifest/src/config.rs:430-434`) も既存パターンに沿う
- **scope union**: `build_scope_with_memory` (`crates/pod/src/pod.rs:2874-2882`) が `deny_write_rules` と `skill_dir_read_rules` を `extend` する形は `scope::deny_write_rules` の参照モデルと完全に対称
- **WorkflowRegistry**: `merge_skill` の「first-fed wins、再ランクしない」契約は doc コメントで明示されており、`ingest_skills` 側が priority 順 feed の責任を負う。registry の不変条件を呼び出し側が守る、よくある分離
- **push_notify**: `[Skill shadowed]` prefix は既存の `[<Domain> ...]` 文体と整合。`Method::Notify` の既存ルートを再利用しており、新たな event 種別の追加は無い
- **Cargo deps**: `serde_yaml` / `tempfile` / `tracing` / `thiserror` はすべて memory crate に既存、`cargo add` 縛りに抵触する追加なし

#### 範囲外項目への侵入チェック
- `allowed-tools` 実効化 — 実装無し。`SkillFrontmatter::allowed_tools` は受信し warn して捨てる (`crates/memory/src/skill.rs:156-161`)、受け皿契約は `tickets/permission-extension-point.md` に記載
- `scripts/` 自動実行 — 無し。Read 経由の閲覧のみ
- ビルトイン skill (`$insomnia/skills/`) — 実装無し
- skill 自動生成 / skill 間依存 — 実装無し
- 新規 protocol message / event 種別 — 無し

新仕様の Scope に過剰実装は無い。

#### 設計変更の波及確認 (`WorkflowSource` 単一バリアント化)

旧 2 バリアント (`WorkspaceSkill` / `UserSkill`) を `Skill { dir }` に統合した変更の影響範囲を確認:

- `WorkflowSource::label()` (`crates/memory/src/workflow.rs:38-46`) は `ShadowedSkill::message()` でしか使われない。旧仕様では「shadow されたのは workspace skill か user skill か」を一行で区別したかったが、新仕様では skill ソースは 1 種類で十分。`label()` の戻り値も `"workspace workflow"` / `"skill"` の 2 値で、メッセージ文字列内で `dir` を `display()` するため出所も特定可能。**user/workspace 区別が必要だった他箇所は無い**
- `permission-extension-point.md` (受け皿チケット) で「`WorkflowRecord` の出所は `WorkflowSource::Skill { dir }` で識別済み」とすべき箇所 — 受け皿実装時の足がかり情報として旧 2 バリアント名で書かれている
- `crates/manifest/src/lib.rs` の `SkillsConfig` および `pub skills: Option<SkillsConfig>` の doc コメント — 旧仕様の「user-level `$config_dir/skills/` is always probed regardless of this field」「in addition to the user-level `$config_dir/skills/`」が残存

#### 不必要な抽象 / 過剰な API のチェック
- `SkillRecord` を一旦経由してから `into_workflow_record` で `WorkflowRecord` に投影する 2 段構造は当面 SkillRecord 独自情報を持たないが、Permission 層が来たときに `allowed_tools` 等 skill 固有 metadata を保持する受け皿として機能する。許容範囲
- `WorkflowSource::label()` は `ShadowedSkill::message()` の 1 caller のみ。`message()` 内に inline しても良いが、`label()` を独立メソッドとして晒しても害は無く、将来 UI / log で再利用される可能性は残せる。許容範囲

### テストカバレッジ

ticket の完了条件にある「frontmatter 検証 / Workflow マッピング / 衝突解決 / manifest 未指定時 0 件」を順に確認:

- frontmatter 検証: `parses_minimal_skill` / `name_dir_mismatch_is_error` / `invalid_slug_name_is_error` / `empty_description_is_error` / `description_at_cap_is_accepted` / `description_over_cap_is_error` / `missing_frontmatter_is_error` / `extra_frontmatter_fields_are_kept` (`crates/memory/src/skill.rs`) — 過不足なし
- Workflow マッピング: `into_workflow_record_uses_skill_defaults` (`crates/memory/src/skill.rs:415-435`) で `model_invokation`/`user_invocable`/`requires`/`source` を確認
- 衝突解決:
  - 内製 Workflow > skill: `merge_skill_shadows_existing_workflow` (`crates/memory/src/workflow.rs:401-428`)
  - skill ↔ skill (列挙順 = 優先順): `merge_skill_first_fed_wins_on_collision` (`crates/memory/src/workflow.rs:431-467`)
  - 衝突メッセージ: `shadow_message_is_human_readable` (`crates/memory/src/workflow.rs:470-484`)
- manifest 未指定時:
  - `ingest_skills_returns_empty_when_skills_section_missing` (`crates/pod/src/pod.rs:3043-3050`)
  - `skill_dir_read_rules_empty_when_skills_section_missing` (`crates/pod/src/pod.rs:3036-3041`)
- 通常経路: `ingest_skills_loads_from_workspace_directories` (`crates/pod/src/pod.rs:3052-3071`)

設計変更で「user vs workspace の優先順位」を verify するテストは不要となり (Round 1 の `merge_skill_priority_workspace_over_user` 相当が削除)、代わりに「first-fed wins 契約」を verify するテストに置き換えられている。新契約に整合的。

`cargo test --workspace --no-fail-fast` 全 crate で 0 failed を確認。

### Round 1 の指摘事項の引継ぎ

- ✅ **解消**: 「scope.allow が常に user_skills_dir を含む / opt-out 手段が無い」 — `paths::user_skills_dir()` が削除され、manifest 未指定時に scope への union も発生しない (`skill_dir_read_rules` の早期 return)。`[skills]` を書かない = 完全 opt-out
- ✅ **解消**: 「Round 1 nit: `pod.rs` の `Permission` import 整形」 — 新コード (`crates/pod/src/pod.rs:2880, 2895-2898`) では `Permission::Read` を直接参照しており、import の整形問題は発生していない
- 引継ぎ (Round 2 でも有効):
  - **`merge_skill` の事前条件依存性**: 「降順 priority feed を呼び出し側が守る」契約は doc + テストで明示されているが debug assertion 無し。`ingest_skills` 以外の caller が増えたときに気付きにくい。follow-up で防衛コードを足す余地はある (non-blocking)
  - **`Slug::parse` が agent-skills 仕様より厳しい**: 仕様は `[a-z0-9-]+` だが内製 `Slug` は先頭末尾 `-` と `--` を禁止。実用的問題は無いが docs/plan で明示する価値あり
  - **`drain_skill_shadows` を resume 時にも emit する仕様の暗黙性**: `crates/pod/src/pod.rs:2501` で resume 時にも shadow Notification を再注入する設計。registry を再構築する以上一貫しているが、ユーザー観点では一行コメントを残しておくと将来の再評価が楽
  - **shadow 経路の end-to-end テスト**: `merge_skill` の戻り値検証はあるが `Pod::from_manifest` → `push_notify` 到達の統合テストは無い (致命的でない)

## 指摘事項

### Blocking

なし。

### Non-blocking / Follow-up

- **stale doc コメント (manifest crate)** — `crates/manifest/src/lib.rs:47-62` が旧仕様前提のまま:
  - L47-48: 「in addition to the user-level `$config_dir/skills/`」
  - L57-62: `SkillsConfig` の doc が「Off by default at the workspace level; user-level `$config_dir/skills/` is always probed regardless of this field.」と明確に誤った仕様を述べている

  動作には影響しないが、将来 `skills` を触る開発者が誤読する。書き換え推奨 (例: 「`directories` で明示指定したパスのみ ingest される。デフォルト ingest は無い」)
- **stale 参照 (cross-ticket)** — `tickets/permission-extension-point.md:61` が旧 2 バリアント名 (`WorkflowSource::WorkspaceSkill { dir }` / `UserSkill { dir }`) で受け皿実装の足がかりを書いている。本チケットの責務外だが、本チケットの設計変更に伴って整合性が崩れた点なので follow-up で受け皿チケット側を更新するのが筋
- **Round 1 から引き継ぐ非 blocking 事項** — `merge_skill` 防衛 assert / `Slug` 厳格化の docs 周知 / resume 時 shadow 再注入のコメント / shadow E2E テスト (本セクション「Round 1 の指摘事項の引継ぎ」末尾参照)

### Nits

- `crates/memory/src/skill.rs:316-326` (`invalid_slug_name_is_error`): `BAD-Caps` 入力の `InvalidName` / `NameDirMismatch` どちらが先に発火するかをテストで明示しておくと validation 順序を変えたときの regression を捕まえやすい (Round 1 から継続)
- `crates/pod/src/pod.rs:3016` のテスト名 `skill_dir_read_rules_lists_workspace_skill_directories` は新仕様だと `workspace_skill` という呼称が違和感ある (skill ソースに workspace/user 区別はもう無い)。`..._lists_manifest_directories` 等への rename を検討 (cosmetic)

## 判断

**Approve** — 設計変更後の完了条件 6 点すべてが対応する実装・テストで verify されており、scope 外項目に踏み込んでいない。`WorkflowSource` 単一バリアント化の波及は本実装内で閉じており、user/workspace 区別が本質的に必要だった箇所は無い。Round 1 で挙げた「scope に常時 user_skills_dir が乗る」「opt-out 手段が無い」という最大の懸念は設計変更で構造的に解消した。Non-blocking として残るのは stale doc コメント (manifest crate / 受け皿チケット) と Round 1 から引き継いだ補強候補で、いずれも別 commit / 別チケットで巻き取って良い性質。
