# システムプロンプトテンプレート

Pod のシステムプロンプトは minijinja テンプレート。first turn 直前に1回だけ render され、以降のターンおよび compact 後も同じ文字列を使い続ける。

## マニフェスト記法

既存の `[worker].system_prompt` をそのままテンプレート文字列として解釈する。新フィールドは無い。

```toml
[worker]
system_prompt = """
You are operating in {{ cwd }}.
Today is {{ date }}.

{{ scope.summary }}

Available tools: {{ tools | join(", ") }}
{% if files.agents_md is defined -%}

{{ files.agents_md }}
{%- endif %}
"""
```

構文は minijinja (Jinja2 互換) のサブセット。未定義変数の参照は `UndefinedBehavior::Strict` により render エラーになる。`{{` のリテラル出力は `{{ '{{' }}` で逃がす。

## 組み込み変数

| キー | 型 | 内容 |
|---|---|---|
| `date` | string | `YYYY-MM-DD` (UTC) |
| `time` | string | `HH:MM:SS` (UTC) |
| `datetime` | string | RFC3339 (UTC, 秒精度) |
| `cwd` | string | Pod の絶対 pwd |
| `scope.readable` | list\<string\> | allow ルールの全パス (Read 以上) |
| `scope.writable` | list\<string\> | allow ルールのうち Write のパス |
| `scope.summary` | string | "Readable:\n  - ...\nWritable:\n  - ..." 形式の整形済み文字列 |
| `tools` | list\<string\> | 登録済みツール名の sort 済み一覧 |
| `files` | map\<string, string\> | 外部ファイル。AGENTS.md 等の供給先として予約。空の場合もあるので `is defined` でガードする |

`files` は常に存在する (本体が空 Map のことはあっても `files` 自体が未定義にはならない) が、個別キー (`files.agents_md` 等) は供給元次第で未定義になり得る。

## 評価モデル

テンプレートの実体化 (render) は遅延評価。プロジェクト内の他の遅延初期化パターン (tool factory / hook builder) と同じ形に揃えている。

### タイミング

1. `Pod::from_manifest` 時点: テンプレート文字列を `SystemPromptTemplate::parse` で **構文検査のみ** 行い、`Pod.system_prompt_template: Option<SystemPromptTemplate>` に保持する。この時点で Worker 側の system_prompt は `None`、session log もまだ書かれない (`head_hash: None`)。

2. `Pod::run` / `Pod::resume` 初回呼び出し冒頭 (`ensure_system_prompt_materialized`):
   1. `worker.tool_server_handle().flush_pending()` で pending な tool factory を materialize して tool 名を確定させる
   2. 現在時刻・cwd・scope・tool 名を集めて `SystemPromptContext` を作る
   3. `template.render(ctx)` で文字列化して `worker.set_system_prompt(rendered)` を呼ぶ
   4. `Pod.system_prompt_template = None` (`Option::take()` で構造的に1回性を保証)

3. その直後の `ensure_session_head` が `head_hash` を見て初回なら `create_session_with_id` を呼び、materialize 後の system_prompt を **SessionStart ログエントリに焼き付ける**。

### 1回性の保証

- `Pod.system_prompt_template` は `Option<SystemPromptTemplate>` で、materialize 時に `take()` する。2 ターン目以降はフィールドが `None` なので `ensure_system_prompt_materialized` は早期 return し、再 render は発生しない。
- compact は Worker の system_prompt フィールドを触らない (pod.rs の `compact` は `w.get_system_prompt()` を読み取って新セッションに引き継ぐだけ)。そのため compact 前後で同じ文字列が流れ続ける。
- 統合テスト `compact_preserves_system_prompt` が実装で直接検証している。

### 責務分離

テンプレート機構は **Pod 層** に閉じる。`llm-worker` はテンプレートの存在を知らず、`Worker::set_system_prompt(String)` で render 済みの文字列を受け取るだけ。llm-worker 側に入った唯一の変更は `ToolServerHandle::flush_pending` を `pub` に昇格させたこと (tool 名を先取りするため)。

## エラー処理

- **構文エラー**: `Pod::from_manifest` の parse 段階で検出 → `PodError::InvalidSystemPromptTemplate { source: SystemPromptError::Parse }` で Pod の生成自体が失敗する。
- **render エラー** (未定義変数など): first turn 直前の `ensure_system_prompt_materialized` で検出 → `PodError::SystemPromptRender { source: SystemPromptError::Render }` で初回 `Pod::run` が失敗する。
- フォールバックはしない。どちらも fail-fast で Pod 起動を止める。

## 供給元の拡張

`SystemPromptContext.files: BTreeMap<String, String>` は本チケットの範囲では常に空だが、key 空間として予約してある。AGENTS.md 取り込み (別チケット) では `ensure_system_prompt_materialized` 内で `files` を埋めるだけで拡張できる。テンプレート側・エンジン側の変更は不要。

## 関連ファイル

- `crates/pod/src/system_prompt.rs` — `SystemPromptTemplate` / `SystemPromptContext` / `SystemPromptError`
- `crates/pod/src/pod.rs` — `Pod.system_prompt_template` フィールド、`ensure_system_prompt_materialized`、`ensure_session_head` の初回 append ロジック
- `crates/manifest/src/scope.rs` — `Scope::summary` / `readable_paths` / `writable_paths`
- `crates/session-store/src/session.rs` — `create_session_with_id` (事前生成 ID で SessionStart を書くための entry point)
- `crates/llm-worker/src/tool_server.rs` — `ToolServerHandle::flush_pending` (pub)
