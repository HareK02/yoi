# Pod 内部プロンプトのカタログ化

## 背景

Pod は Worker を拡張する機構を持つ: コンテキスト圧縮 (Compact)、非同期通知 (Notify)、中断と再開 (Interrupt)、system prompt の trailing section、AGENTS.md 取込時の注記。これらの機構がランタイムで Worker に注入する/Worker 向けに構成するプロンプトは、現状各モジュール内の `const &str` / `format!` に分散している。

| 場所 | 役割 |
|---|---|
| `crates/pod/src/pod.rs:50` `SUMMARY_SYSTEM_PROMPT` | Compact worker の system prompt |
| `crates/pod/src/notification_buffer.rs:73` `format_notification` | `Method::Notify` を Worker history に system_message として注入するラッパー |
| `crates/pod/src/interrupt_and_run.rs:18-20` | 中断時の synthetic tool_result と system_message |
| `crates/pod/src/system_prompt.rs:179-203` `append_trailing_section` | `## Working boundaries` / `## Project instructions (AGENTS.md)` ヘッダとレイアウト |
| `crates/pod/src/agents_md.rs:19` `TRUNCATION_NOTICE` | AGENTS.md が 64KB 超過したときの末尾注記 |

`resources/prompts/default.md` でユーザー向け instruction テンプレートは一元化された一方、Pod が Worker を拡張する側のプロンプトは並行した一元管理を持たない。結果として:

- 全体を俯瞰しづらく、新しい injection 点を足すときに既存との一貫性が取れない
- 多言語化や文体カスタマイズの導線がない
- builtin から差分だけ書き換える軽量な手段がない

tool description は tool 宣言と併置が自然（tool 追加 = 1 箇所追加）なので本チケット対象外。Pod の Worker 拡張機構は異質なものが「Pod のトーン」として束になって振る舞う性質上、中央で扱う価値がある。

## 要件

### 1. 中央モジュール: `PodPrompt` enum

Pod が Worker に注入する全プロンプトを列挙する enum を 1 つ置く。variant の集合が「存在する injection 点」の master。新しい注入点を増やすときは variant 追加が必要 = コード上で勝手に散らかせない。

Pod 内部の各モジュールは `PodPrompt::CompactSystem.render(&ctx)` のように 1 本の API で引く。直接 `include_str!` / `const &str` / `format!` で prompt 文字列を書かない。

### 2. 翻訳パック形式: `prompts.toml`

全 variant を key-value で持つ TOML。値は minijinja テンプレート文字列（builtin/ランタイム問わず一律 minijinja render）。

```toml
[prompt]
interrupt_system_note = "[The previous turn was interrupted by the user. The user's next request follows.]"

notify_wrapper = """[Notification]
{{ message }}

This is a notification, not a blocking request. ..."""

# 長文は外部ファイルに link
compact_system = "{% include '$insomnia/internal/compact_system.md' %}"
```

変数展開は各 variant ごとに定義された context を render 時に渡す（例: `notify_wrapper` は `message`、`compact_system` は tool 名リストなど）。context の具体形は実装段階で variant ごとに確定する。

### 3. Builtin pack の網羅性をビルドエラーで強制

`resources/prompts/internal.toml` が `PodPrompt` 全 variant を網羅していないとビルド失敗。enum に variant を足したが builtin pack に key が無い、あるいは逆、はコンパイルが通らない。`build.rs` もしくは proc-macro で検査する（どちらを取るかは実装時判断）。

### 4. 4 段の置換マージ

key 単位で overlay。下層から順に apply、後勝ち:

```
builtin           resources/prompts/internal.toml                 (必須・網羅)
  ↓
user              $XDG_CONFIG_HOME/insomnia/prompts.toml          (任意・auto)
  ↓
workspace         <project>/.insomnia/prompts.toml                (任意・auto)
  ↓
manifest pack     manifest.pod.prompt_pack で指名                (任意)
```

- 欠落 key: 下層から継承
- ランタイム層 (user/workspace/manifest pack) の unknown key: `tracing::warn!` して無視
- builtin 層の不整合はビルドエラー（前項）

### 5. 値 render は minijinja 統一、include は既存 prefix resolver 流用

全値を minijinja で parse / render。`{% include "$prefix/..." %}` によって外部ファイルを link 可能。resolver は既存 `crates/pod/src/prompt_loader.rs` を流用し、`$insomnia` / `$user` / `$workspace` すべてをどの層の pack からも参照できる。

### 6. manifest 露出

```toml
[pod]
prompt_pack = "$user/packs/japanese.toml"
```

任意フィールド。指定されていれば 4 段目 overlay として適用。auto-discovery (user/workspace) とは独立で共存する。子 Pod を spawn する際、親が子の役割に応じた pack を明示する用途を想定。

## 設計判断

### prompt_pack は prefix 名前空間に載せない

既存 `$insomnia/` / `$user/` / `$workspace/` は**名前空間**で、同じ key で複数の source を指し合うことはない。一方 pack は**レイヤー**で、同じ key を置き換える。この 2 つを同じ軸に混ぜるとユーザーが「どの書き方で上書きされるか」を予測できなくなる。

pack ファイルは**固定パスの auto-discovery** と **manifest による明示指名**の 2 経路のみ。各値内部で `{% include "$prefix/..." %}` を使うのは別軸なので prefix 体系の利点はそのまま享受できる。

### 長文ファイル分離用の独立フィールドを作らない

値が minijinja である以上、長文を別ファイルにしたければ `"{% include '$insomnia/internal/foo.md' %}"` と書けば済む。「TOML 値の文字列」と「ファイル参照」の 2 値型を用意する必要はない。1 種類で統一する。

### ランタイム層の unknown key は warn

pack ファイルを書いた時点と Pod バージョンがずれたとき、hard error にすると古い pack で Pod が起動しなくなる。前方互換のため warn で無視する。builtin 層は build-time に同梱するので不整合はビルドで捕まえられる = error で問題ない。

### tool description は本チケット対象外

tool 追加は tool ごとの 1 箇所の自然な単位で、description は tool の属性として宣言と併置が読みやすい。Pod 内部 injection は「異質なものが一体としてトーンを決める」共通軸なので中央化の価値が別にある。この差を混ぜない。

### auto-discovery と manifest 指定を両立させる

auto-discovery は「ユーザー or プロジェクトの永続設定 (翻訳、文体)」、manifest 指定は「**その Pod の役割**による差し替え」と目的が別。どちらか一方では片方のユースケースが潰れるので両立する。key 単位の merge は共通なので実装コスト差は小さい。

## Scope 外

- tool description の resources 化（併置方針を維持）
- 具体的な翻訳 pack の作成（本チケットは導線のみ）
- pack 編集 GUI / TUI
- user/workspace 以外の auto-discovery パス追加（別 XDG 層、bundle pack 等）

## 依存

- `crates/pod/src/prompt_loader.rs` (`$prefix` resolver を minijinja include から流用)
- `crates/pod/src/system_prompt.rs` (minijinja 使用パターン)
- `crates/manifest`: `pod.prompt_pack: Option<String>` 追加のみで破壊的変更なし

## 影響範囲

- `crates/pod/src/prompts.rs` (新設): `PodPrompt` enum、render API、pack loader、4 段 merge
- `resources/prompts/internal.toml` (新設): builtin pack
- `resources/prompts/internal/*.md` (新設): 長文外出し
- `crates/pod/` 各モジュール: 既存ハードコードを `PodPrompt::...render(&ctx)` 呼び出しに置換
- `crates/pod/build.rs` もしくは proc-macro (新設): enum ⇔ builtin pack 網羅検査
- `crates/manifest/src/config.rs`: `prompt_pack` フィールド追加

## 実装順序

1. `PodPrompt` enum と render API を定義。builtin map (in-memory、`include_str!`) から引くだけの最小実装。variant の render 網羅を単体テストで確認
2. `resources/prompts/internal.toml` に全 key を書き、既存ハードコードをそのまま文字列として移植。各モジュールの呼び出しを `PodPrompt::...render` に置換 (挙動は既存と完全同一)
3. builtin 網羅を build-time 検査に格上げ (enum ⇔ pack の双方向で欠落/余剰を検出)
4. user / workspace の auto-load と 3 段マージ。ランタイム unknown key の warn
5. `manifest.pod.prompt_pack` を追加、4 段目 overlay として load
6. 長文 variant (compact_system など) を `{% include "$insomnia/internal/..." %}` 形式に分離。`$user` / `$workspace` から include で override できることをテスト

各ステップ終了時点でビルド通過・既存テスト合格を維持する。
