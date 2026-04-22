# Review: Pod 内部プロンプトのカタログ化

## 前提・要件の確認

1. **`PodPrompt` enum (要件1)** — 満たされている。7 variant を列挙し (`crates/pod/src/prompts.rs:60-81`)、各 variant が `key()` とともに declaration-order の `ALL`/`KEYS` 定数に連動する。呼び出し側 (pod/notification_buffer/pod_interceptor/interrupt_and_run/system_prompt/agents_md) はすべてカタログ経由に置換済み。`grep` で `SUMMARY_SYSTEM_PROMPT` / `TRUNCATION_NOTICE` / ハードコード文字列は production コードから一掃されている (テスト文字列のみ残存)。

2. **`internal.toml` builtin pack (要件2)** — 満たされている。`resources/prompts/internal.toml` に 7 key 全てを配置 (`resources/prompts/internal.toml:10-35`)。minijinja テンプレートとして評価され、`compact_system` は `{% include "$insomnia/internal/compact_system" %}` 経由で外部 `.md` を参照。長文分離の設計判断「`include` 一本化」は忠実に実装されている。

3. **ビルド時網羅性検査 (要件3)** — 満たされている。`build.rs` が TOML を parse して `INTERNAL_KEYS` slice を emit、`prompts.rs:122-145` の `const _: ()` が `PodPrompt::KEYS ↔ INTERNAL_KEYS` を双方向で検査する。ユーザー報告通り、片側除去/余剰で `panic!` メッセージ 2 種が発火することを確認済み。const eval ベースで proc-macro を回避しているのは軽量かつ依存最小で筋が良い。

4. **4 段 overlay merge (要件4)** — 満たされている。`PromptCatalog::load` が builtin → user (`user_pack_file`) → workspace (`workspace_pack_file`) → manifest pack の順で `merge_into` を呼ぶ (`prompts.rs:260-284`)。未知 key は `tracing::warn!` + ignore (`prompts.rs:370-386`)。builtin 層の不整合は build 時 error (要件3 と一体)。テスト `user_pack_overrides_builtin` / `workspace_pack_wins_over_user_pack` / `manifest_pack_wins_over_workspace_pack` / `unknown_key_in_runtime_pack_is_ignored_with_warning` が precedence と warn-ignore を裏打ちしている。

5. **minijinja 統一 + prefix resolver 流用 (要件5)** — 満たされている。`build_catalog` (`prompts.rs:440-482`) が `Environment` の `path_join_callback`/`loader` に既存 `PromptLoader` を配線し、値内の `{% include "$prefix/..." %}` が `$insomnia` / `$user` / `$workspace` 全てから引ける。テスト `value_can_pull_long_text_via_include` が builtin の `compact_system` を runtime pack 側から再参照できる挙動を確認している。

6. **`manifest.pod.prompt_pack` (要件6)** — 満たされている。`PodMeta.prompt_pack: Option<String>` を `#[serde(default)]` で追加 (`crates/manifest/src/lib.rs:36-46`)、`PodMetaConfig` のカスケード merge にも反映 (`crates/manifest/src/config.rs:43, 199-206, 340, 415`)。`Pod::from_manifest` / `from_manifest_spawned` は `PromptCatalog::load(&loader, manifest.pod.prompt_pack.as_deref())` を呼ぶ。`$user/` プレフィックス経由で解決するテスト (`manifest_pack_supports_user_prefix`) 付き。

## アーキテクチャ・スコープ

- **レイヤー境界** — 変更は `crates/pod` と `crates/manifest` (フィールド追加のみ) に収まり、`llm-worker` は触っていない。Pod 内部プロンプトは Pod 層で扱う、というスコープ方針を守っている。
- **prefix × layer の分離方針** — 設計判断「prefix 名前空間 (resolve where) と layer (merge precedence) は混ぜない」が正しく反映されている。pack 自体は **固定パスの auto-discovery** と **manifest での明示指名** の 2 経路に限定され、`PromptLoader` の prefix 名前空間 (`$insomnia`/`$user`/`$workspace`) は pack 値内部の `{% include %}` でのみ再利用される。
- **ランタイム vs ビルド時エラーの使い分け** — builtin 不整合は const-eval panic、runtime pack の unknown key は `warn + ignore`。前方互換性の判断通り。
- **tool description に手を入れていない** — scope 外宣言を遵守。`resources/prompts/internal/` ディレクトリは builtin 長文のみ (`compact_system.md` 一件)。
- **cargo add 運用** — `[build-dependencies] toml = "1.1.2"` が追加されているが、`cargo add --build` 経由で追加されているかは diff からは直接確認できない。既に `[dependencies]` に `toml = "1.1.2"` が存在するので workspace の既存バージョンに揃っており、挙動上の懸念は無い (手動編集だったとしても結果は同じ)。
- **影響範囲との一致** — ticket の「影響範囲」リスト (`prompts.rs` 新設 / `internal.toml` / `internal/*.md` / 呼び出し置換 / `build.rs` / `manifest/config.rs`) はすべて diff 上に存在。未対応の項目は無い。

## 指摘事項

### Blocking
- なし。

### Non-blocking / Follow-up

- **[API 表面の二重化]** `PromptCatalog::render(PodPrompt, Value)` と `compact_system()` 等の typed accessor が同時に `pub` 公開されている (`prompts.rs:289-342`)。ticket は `PodPrompt::CompactSystem.render(&ctx)` を 1 本の API として期待していた。実装の「variant ごとに context 型が固定なので typed accessor が筋」という判断は妥当で、現状の呼び出し元もすべて typed を使っている。`render(PodPrompt, Value)` を `pub(crate)` まで降格するか、typed accessor だけを公開面として残す方が、将来「新しい variant を追加したら typed accessor も実装しないとコンパイル的には気づかない」という弱さを防げる。
- **[`append_trailing_section` の可視性]** `pub fn append_trailing_section` (`system_prompt.rs:188`) は module 内でしか呼ばれていない。`pub(crate)` か private へ落として API 表面を絞るのが望ましい。
- **[factory 側の `.is_file()` と catalog 側の `.is_file()` の二重フィルタ]** `PodFactory::build_prompt_loader` (`factory.rs:202-211`) が既に `.is_file()` で絞った `PathBuf` を渡すのに、`PromptCatalog::load` も `path.is_file()` を再チェックしている (`prompts.rs:267,273`)。冗長で、仕様として「渡されたら読む」なのか「存在チェックは catalog 側の責務」なのかが曖昧。後者に寄せるなら loader 側は無条件に `PathBuf` を渡し、catalog 側だけで分岐させる (既存の挙動を保つ)。前者にするなら loader 側は「pack file が無ければ `None`」という契約を DocComment 化する。ticket にとって致命傷ではない。
- **[`$insomnia/` manifest pack の `.toml` 読み取り経路]** `load_raw_builtin` は拡張子を付けない raw loader だが、これは元々 `$insomnia/` prefix が `.md` 前提で設計されていた `PromptLoader` を「拡張子ごと渡せば読める `.toml` 用 shortcut」として拡張している (`prompt_loader.rs:175-204`)。`$insomnia/` prefix の 2 つの意味 (テンプレートとしての `.md` 参照と、pack ファイルとしての `.toml` 参照) がひとつのローダに同居する形になっている。機能上は問題ないが、`$insomnia/default` (`.md` 付く) と `$insomnia/internal/foo.toml` (拡張子必須) で path の書き方が視覚的に揺れる。ドキュメント or 命名で区別してもよい。

### Nits

- `PromptCatalog::builtins_only()` は `load(&PromptLoader::builtins_only(), None)` を thin-wrap するだけだが、テスト用に便利なので残しておいて問題なし (多用されている)。
- `CatalogError::UnknownKey` は `PromptCatalog::render` が未登録テンプレートを引いた場合用だが、現状 `PodPrompt::key()` 経由でしか呼ばれないので到達しづらい。将来外部入力を受ける経路を増やしたときに活きる。
- `single` 関数 (`prompts.rs:345-350`) は `BTreeMap<&'static str, Value>` を毎回組み立てている。typed accessor の呼び出し頻度 (LLM request ごとに高々数回) ではマイクロ最適化する必要はない。

## 判断

**Approve** — 要件 1〜6 はすべて満たされ、設計判断 (prefix × layer 分離 / 単一値型 / ビルド時 vs ランタイム分岐 / tool description 非対象 / auto + manifest 両立) が正確に実装されている。ワークスペーステストは全緑。Non-blocking な API 整理の余地 (render の可視性、loader-catalog 間の is_file 二重チェック) はあるが、ticket を閉じる上の障害ではない。
