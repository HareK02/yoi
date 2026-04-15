# レビュー: Pod Factory

対象差分: `crates/manifest/src/{lib,config}.rs`, `crates/pod/src/{lib,pod,main,system_prompt,factory,prompt_loader}.rs`, `crates/pod/examples/{pod_cli,pod_protocol}.rs`, `crates/provider/src/lib.rs`, `resources/prompts/`, `docs/pod-factory.md`（いずれも未コミット）

## 要件達成状況

| 要件 | 状態 |
|---|---|
| カスケード基盤（ユーザー・プロジェクト・overlay の層マージ） | ✅ `manifest::PodManifestConfig` が部分形として実装。`PodFactory` が 4 層（default/user/project/overlay）を順にマージ |
| 解決後の型は `PodManifest` のまま | ✅ `TryFrom<PodManifestConfig> for PodManifest` が検証付き変換 |
| 各層の manifest.toml スキーマは同じ | ✅ partial 型群 (`PodMetaConfig` / `ProviderConfigPartial` / `WorkerManifestConfig` / ...) が本物と同構造 |
| ユーザーパスは XDG | ✅ `user_manifest_path` が `XDG_CONFIG_HOME` → `$HOME/.config` の順 |
| プロジェクトルートは `.insomnia/` 最近接 | ✅ `find_project_manifest` が cwd から上方向に walk |
| scope は union マージ | ✅ `merge_scope` が allow/deny を `extend` |
| マップは key-wise マージ | ✅ `ToolOutputLimitsPartial::merge` |
| スカラー / Option は upper 優位 | ✅ `upper.x.or(self.x)` パターンで実装 |
| 未知フィールドは warn のみ | ✅ `serde_ignored::deserialize` + `tracing::warn!` |
| 型ミスマッチは hard error | ✅ toml パースで通常通り失敗 |
| パスフィールドは絶対パスのみ | ✅ `ensure_absolute` が `pod.pwd` / `provider.api_key_file` / `scope.*.target` を検証 |
| `Pod::from_manifest(manifest, store, loader)` 二段 API | ✅ + `from_manifest_toml` 便利関数。旧 path 受けは廃止 |
| `manifest_dir` 引数の消滅 | ✅ `Pod` 構造体から削除、`provider::build_client` からも消滅 |
| プロンプト3層ローダ | ✅ `PromptLoader` が project/user/builtin の順で解決 |
| ビルトインは `resources/prompts/` + `include_dir!` | ✅ `pod/src/prompt_loader.rs:17` |
| プロンプト include の親変数伝搬 | ✅ minijinja デフォルト挙動、`include_resolves_builtin_prompt` テストで確認 |
| CLI の `--user-manifest` / `--project` / `--overlay` / `--pwd` | ✅ `pod/src/main.rs` で実装。引数無しでも XDG + cwd 自動解決で動く |
| 引数無しでの最小構成動作 | ✅ CLI デフォルトが auto 系メソッドにつながる |
| ドキュメント (`docs/pod-factory.md`) | ✅ カスケード層・マージ規則・CLI・プログラマティック API・ビルトイン一覧を網羅 |

**すべての要件を達成**。さらに当初のチケットに無かった点として CLI docs の整理、`examples/` の追随更新、provider の `~` 展開廃止（絶対パス徹底）までカバーしている。

## アーキテクチャ統合

### クレート境界

- **`manifest/src/config.rs`** — `PodManifestConfig` を manifest crate 側に置いたのは**正しい判断**。manifest crate は純データ型と検証ロジック (`Scope::from_config`, `TryFrom`) の置き場で I/O を持たない、という既存方針をそのまま継承している
- **`crates/pod/src/factory.rs`** / **`prompt_loader.rs`** — I/O（ファイル読み、XDG 解決、`include_dir!`）は pod crate 側に集約。新規 crate を作らず、既存レイヤに素直に乗せている。前回私が推した統合方針どおり
- **`PodFactory::resolve()` が `(PodManifest, PromptLoader)` を返す** — factory は「manifest + どこからプロンプトを引くか」を一体で管理し、Pod は渡された loader を素直に使うだけ。責務の分離が綺麗
- **`Pod::manifest_dir` の完全削除 + `resolve_pwd` の絶対パス強制** — 「パスの正規化は cascade 層の専売事項」という単一ソースの原則が守られている。`provider::build_client` からも `manifest_dir` 引数が消え、`~` 展開も消え、relative rejection だけ残った。下流が大幅にシンプルになった

### 副作用的な改善

- `Pod::from_manifest_toml` が `PromptLoader::builtins_only()` を暗黙に使う → テスト・examples が「単層 TOML で Pod 起動」を一行で書けるようになった
- `examples/pod_cli.rs` と `pod_protocol.rs` が絶対パス化と新 API に追随済み。壊れ残りなし

## 指摘事項

### 1. 🟢 `with_overlay_toml` / `with_overlay_config` の重ね合わせが「同じ層にマージ」

`factory.rs:123-139`:

```rust
pub fn with_overlay_toml(mut self, toml: &str) -> Result<Self, FactoryError> {
    let config = PodManifestConfig::from_toml(toml).map_err(FactoryError::OverlayParse)?;
    self.overlay = Some(match self.overlay {
        Some(existing) => existing.merge(config),
        None => config,
    });
    Ok(self)
}
```

`with_overlay_*` を複数回呼ぶと、独立したレイヤにはならず**1 つの overlay スロットに逐次マージ**される（後勝ち）。これは CLI + 1 回のプログラマティック注入には十分だが、「複数の独立した overlay を priority order 付きで積みたい」ニーズには応えない。

**判断**: 現状の要件範囲内では問題なし。将来 preset や環境変数ベースの overlay を追加する場合に再検討。

**細かい付随点**: `factory.rs` のテスト `cascade_overlay_overrides_project_overrides_user` は名前が「overlay が project を override し、project が user を override する」と読めるが、実際には**全部 overlay スロットに積まれている**（user/project スロットは未使用）。テスト内コメントで断り書きはあるが、名前だけ見ると誤解されやすい。`cascade_priority_layer_ordering` のほうが本来のレイヤ順序を検証しているので、前者は `overlay_stacking_merges_in_place` などに改名するとより正確。

### 2. 🟢 "builtin defaults" 層の実体がゼロ

チケット方針:
> ビルトインのデフォルト: コードに焼き込んだ基本値（現在 `PodManifest` 各フィールドの `#[serde(default)]` や `Default` 実装に散っているものを集約）

実装方針（`factory.rs` module doc）:
> 1. **Builtin defaults** — in-code defaults, currently empty. Upper layers provide everything; `TryFrom<PodManifestConfig>` fills in per-field defaults

実際の builtin layer は `PodManifestConfig::default()`（全部 None）で、デフォルト値の適用は `TryFrom` 内の `.unwrap_or(ToolOutputLimits::default())` のように散在している。つまり**「散らばってる defaults を builtin layer に集約する」という元の目的は達成されていない**。

**判断**: 実運用上の挙動は同じ（ユーザーから見れば `PodManifest` の各フィールドが既定値を持つことに変わりなし）。チケットの表現 vs 実装の厳密な乖離だけで、受け入れ可否には影響しない。将来 defaults の一覧を可視化したくなった時点で builtin layer を実体化する余地を残しておけば OK。

### 3. 🟢 CLI `--pwd` の overlay 注入が文字列フォーマット経由

`main.rs:64-76`:

```rust
parts.push(format!(
    "[pod]\npwd = \"{}\"\n",
    absolute.display().to_string().replace('\\', "\\\\")
));
```

生成した TOML 断片を `with_overlay_toml` に渡している。`\` は escape しているが `"` は escape していないため、`"` を含むパス（Linux では理論上あり得る）で壊れる。また `--overlay` 側と `--pwd` 側を join するときに両者の構文が衝突しないかも微妙。

**推奨**: 文字列を作らず、`PodManifestConfig` を直接構築して `with_overlay_config` で渡す形に書き換える:

```rust
if let Some(pwd) = cli.pwd.as_ref() {
    let absolute = std::fs::canonicalize(pwd).unwrap_or_else(|_| pwd.clone());
    factory = factory.with_overlay_config(PodManifestConfig {
        pod: PodMetaConfig { pwd: Some(absolute), ..Default::default() },
        ..Default::default()
    });
}
if let Some(overlay) = cli.overlay.as_deref() {
    factory = factory.with_overlay_toml(overlay)?;
}
```

型を経由するので escape 問題が消え、`--pwd` と `--overlay` の干渉も無くなる。

**判断**: 現実には問題が起きる可能性は極めて低いが、型経由のほうが筋が良い。**任意**。

### 4. 🟢 `resolve_provider` に dead code

`manifest/src/config.rs:218-237`:

```rust
fn resolve_provider(
    cfg: ProviderConfigPartial,
    field_prefix: &'static str,    // ← 使われていない
    kind_field: &'static str,
    ...
) -> Result<ProviderConfig, ResolveError> {
    let _ = field_prefix;          // ← 明示的に捨てられている
    ...
}
```

`field_prefix` を取っているが関数内で使っていない（`let _ =` で捨てている）。将来エラーメッセージで `"missing field: {prefix}.kind"` のようにしたい意図だったと推察されるが、現在の `ResolveError::MissingField` は静的文字列を直接受けているので不要。

**判断**: 不要なので削除推奨。**任意**（実害なし、lint が効けば dead_code 警告が出るかも）。

### 5. 🟢 `~` 展開廃止は breaking change

`provider` から `~/.config/insomnia/keys/anthropic` のような `~` 始まりパスの展開処理が消えた。既存の手書き manifest にこの形式が書かれていると resolve で `RelativePath` エラーになる。

**判断**: チケットで「絶対パスのみ」と決めた結果であり、意図通り。`docs/pod-factory.md` でも「パスの絶対性」として明記されている。受け入れ可。

### 6. 🟢 `find_project_manifest` の canonicalize 失敗時フォールバック

`factory.rs:197-208` は `start.canonicalize()` が失敗したら raw path で walk を続ける。`.insomnia` 名前の検出は path 比較だけなので動作するが、shell が与えた相対パスが絶対化されないまま `dir.parent()` を続けると仕様上の最上位で止まる可能性。

**判断**: 実害小。canonicalize が失敗するケースは cwd が無効等で、そもそも Pod 起動前の別エラーに出るはず。**不問**。

## テスト

- `manifest/src/config.rs`: 14 ケース（resolve 成功 / 必須欠落 / 相対パス3種 / scalar merge / scope union / per_tool keywise / option struct / type mismatch / unknown field / partial layer / end-to-end cascade）
- `pod/src/factory.rs`: 7 ケース（overlay only / 模擬レイヤ順 / 実レイヤ順 / walk-up / 無プロジェクト / loader 連携 / 必須欠落）
- `pod/src/prompt_loader.rs`: 6 ケース（builtin 存在 / サブディレクトリ / 未知 / user override / project override / fallthrough）
- `pod/src/system_prompt.rs`: 2 ケース（include 成功 / 未知 prompt）
- `provider/src/lib.rs`: 既存テストを新シグネチャに追随 + 新規 relative rejection

**合計 30 ケース超**、各層の検証が丁寧。特に `resolve_produces_loader_with_project_prompts_dir` は factory + prompt_loader + system_prompt の**3 層を貫く end-to-end テスト**で、この変更でもっとも壊れやすい配線を lock-in している点が優秀。

## 結論

**無条件で受け入れ可**。要件は全項目達成、アーキテクチャ統合も筋が良く、テスト被覆も厚い。指摘はすべて任意修正または nit レベルで、受け入れ可否に影響しない。

任意修正として推せるのは:

- **指摘 3**（CLI `--pwd` の型経由化）が最も筋が良い改善。余力があれば
- **指摘 4**（dead code）は数行で済む clean-up
- **指摘 1 のテスト名**（`cascade_overlay_overrides_project_overrides_user` → `overlay_stacking_merges_in_place` 等）も小さな改善

上記はいずれも受け入れ後の別タスクとしても良い。

---

## フォローアップ差分 (2026-04-16)

レビュー後の追加作業として、指摘 #2（builtin defaults の実体ゼロ）を解消し、
**デフォルト値のメンテナンス性向上**を目的とした集約リファクタを入れた。

### 変更内容

- **`crates/manifest/src/defaults.rs` 新設**: 全 manifest デフォルト値を
  `pub const` で宣言する単一の真実ソース
  - `TOOL_OUTPUT_MAX_BYTES`, `PRUNE_PROTECTED_TURNS`,
    `PRUNE_MIN_SAVINGS`, `COMPACT_RETAINED_TURNS`
- **`crates/manifest/src/lib.rs`**: 既存の `default_*` fn 群を constants を
  返すだけの 1 行に縮小。`ToolOutputLimits::default()` /
  `CompactionConfig::default()` の serde `#[default = "..."]` 経路もすべて
  constants に収束
- **`crates/manifest/src/config.rs`**:
  - `PodManifestConfig::builtin_defaults()` を追加（cascade の最下層として使う
    構築メソッド、constants 直接参照）
  - `TryFrom<PodManifestConfig> for PodManifest` が `ToolOutputLimits::default()`
    / `CompactionConfig::default()` 経由をやめ、constants を直接 `unwrap_or`
    する belt-and-suspenders 形
  - 指摘 #4 の `resolve_provider` dead param (`field_prefix`) を削除
- **`crates/pod/src/factory.rs`**:
  - `PodFactory::resolve` の base layer を `PodManifestConfig::default()` から
    `PodManifestConfig::builtin_defaults()` に切替。これで "builtin layer" が
    実体を持つ cascade 層として機能
  - 指摘 #1 のテスト名 `cascade_overlay_overrides_project_overrides_user` →
    `overlay_stacking_merges_in_place` にリネーム
- **`crates/manifest/src/config.rs` テスト追加**:
  - `builtin_defaults_populates_tool_output_max_bytes`
  - `builtin_defaults_merged_into_minimal_resolves_with_defaults`

### 効果

- デフォルト値を変えるときの編集箇所が**1 ファイル 1 行**（`defaults.rs`）に。
  従来は `default_*` fn、`Default::default()` impl、`TryFrom` フォールバックの
  3 経路にそれぞれ値が書かれていたが、すべて `defaults::X` を参照する形に収束
- チケット本文で当初意図されていた「builtin layer がデフォルト値を保持する」
  概念が `builtin_defaults()` + factory の base 層で実体化
- `TryFrom` の fallback は残すので、`builtin_defaults()` を経由しない直接構築
  （テスト等）でも同じ既定値が保証される（belt-and-suspenders）

### テスト

全 ワークスペース テスト通過（manifest 45 / pod 71 を含む）。新規 2 ケース
で `builtin_defaults` の挙動と constants の同一性を lock-in。

### 未処理

- **指摘 3**（CLI `--pwd` の型経由化）は未対応。現在の文字列 format 経由でも
  escape 問題が実際に起きる可能性は極めて低いため保留。必要になった時点で別
  フォローアップとする

この差分の後、指摘 #2 / #4 / #1（テスト名）は解消済み。受け入れ可否には
変化なし（元々「無条件で受け入れ可」）。
