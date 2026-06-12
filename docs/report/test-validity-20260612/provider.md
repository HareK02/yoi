# テスト妥当性レビュー: provider
- 評価: 概ね良い

## 確認した範囲

- Crate: `crates/provider`
- 確認した主な責務:
  - provider/model catalog の読み込みと `ModelManifest` 解決
  - auth 参照から `ResolvedAuth` への解決
  - `LlmClient` 構築エントリポイント
  - Codex OAuth の auth.json 解析、token refresh、header 生成、error mapping
- 読んだファイル:
  - `crates/provider/Cargo.toml`
  - `crates/provider/README.md`
  - `crates/provider/src/lib.rs`
  - `crates/provider/src/catalog.rs`
  - `crates/provider/src/codex_oauth/{mod.rs,auth_json.rs,jwt.rs,refresh.rs,error.rs}`
  - `resources/providers/builtin.toml`
  - `resources/models/builtin.toml`

## 現在のテストがよくカバーしていること

- Catalog resolution は有用な振る舞いレベルでカバーされている。
  - builtin provider/model catalog が parse できる。
  - `ref` 解決が provider catalog と model catalog のフィールドを merge する。
  - inline manifest form が `scheme`、`model_id`、`auth` を要求する。
  - manifest override が catalog default より優先される。
  - unknown provider は hard error になる一方、unknown model は provider default に fallback する。
  - `openrouter/anthropic/claude-sonnet-4.6` のような nested model id がカバーされている。
  - context-window override と backend max clamping がカバーされている。

- User catalog override の振る舞いには意味のあるカバレッジがある。
  - `YOI_CONFIG_DIR` は serial tests で保護されている。
  - provider override は存在する場合 builtin より優先される。
  - override が存在しない場合は builtin に fallback する。
  - malformed provider TOML は silent fallback ではなく parse error を返す。

- Auth resolution tests は重要な local safety property をカバーしている。
  - `SecretRef` resolution は injectable resolver を使う。
  - missing secret は logical id を露出するが、key に見える material は露出しない。
  - API key file loading は whitespace を trim する。
  - relative auth file path は reject される。
  - missing API key は期待される `ApiKeyMissing` を返す。
  - auth-less Ollama-style construction は成功する。

- Codex OAuth tests は local integration として十分に強い。
  - `auth.json` loading は unknown field を保持する。
  - `id_token` JWT からの account id fallback がテストされている。
  - missing auth file は login-oriented error を報告する。
  - persistence は unknown field を保持し、Unix で `0600` を強制する。
  - JWT parsing は `exp`、ChatGPT claims、missing `exp`、malformed JWT をカバーしている。
  - refresh tests は timeout classification と一般的な permanent 401 classifications をカバーしている。
  - provider-level tests は fresh-token headers、FedRAMP header、persistence 付き expired-token refresh、permanent refresh login hint、missing auth file を検証している。

## 不足 / 疑問のあるテスト

- `build_client` / `build_client_from_config` の construction tests は、ほとんどが `is_ok()` または 1 つの error shape を assert している。構築された transport の effective base URL、選択された scheme、model id、capability fallback、Codex-OAuth-specific OpenAI Responses behavior を検証していない。このため、重要な provider-factory invariant のテストが弱い。

- Codex OAuth default base URL path は直接テストされていない。`effective_base_url` には `https://chatgpt.com/backend-api/codex` 用の特別な `AuthRef::CodexOAuth` branch があるが、誤って OpenAI Responses scheme default に fallback しても検知するテストがない。

- Codex OAuth 用の OpenAI Responses special case は、`max_output_tokens`、`temperature`、`top_p` の forwarding を無効化する。これは重要な compatibility invariant だが、provider crate にはそれに対する focused assertion がない。transport internals が opaque なままであれば、狭い test hook か、所有 layer でのより高レベルな request-building test が必要かもしれない。

- Builtin catalog tests は、正確な provider list と count を assert している箇所がやや脆い:
  - `builtin_has_four_providers`
  - `load_falls_back_to_builtin_when_override_absent`
  
  これは偶発的な catalog drift を捕捉できるため価値があるが、正当な provider を追加すると、invariant-oriented というより一部 snapshot-like なテスト更新が必要になることも意味する。意図的であれば許容できるが、pure behavior testing ではなく catalog contract testing として扱うべき。

- User model override behavior は provider override behavior よりカバーが薄い。
  - `models.toml` override precedence に相当するテストがない。
  - malformed `models.toml` parse-error test がない。
  - providers と models は設計上独立して override されるため、対称的なカバレッジが必要。

- Codex OAuth guarded-reload behavior は文書化されているが、直接テストされていない。
  - “file becomes fresh between stale read and refresh” をカバーするテストがない。
  - refresh 前に account id が変わり、そのため refresh を skip するケースをカバーするテストがない。
  - これらは concurrency/race-safety invariants であり、現在のテストは通常の refresh path は検証しているが、race guards は検証していない。

- `last_refresh + 8 days` による Codex OAuth staleness fallback は直接テストされていない。現在の provider tests は access-token `exp` を使って fresh/expired cases を強制している。実際の auth file で malformed/non-JWT access token が想定されるなら、この fallback はテストするだけの重要性がある。

- Refresh HTTP tests は outbound request shape を assert していない。
  - `client_id`
  - `grant_type = refresh_token`
  - `refresh_token`
  - JSON content type
  
  現在の mock は期待される path へ POST が発生したことは証明しているが、Codex-compatible body が正しいことは証明していない。

- Error conversion のカバレッジは間接的である。`CodexAuthError::to_client_error` の permanent-path behavior は provider-level login-message test を通して部分的にカバーされているが、transient refresh と config-classified errors は `ClientError` variants として直接 assert されていない。

- `CodexAuthProvider::from_default_home` の environment precedence（`HOME` より `CODEX_HOME`）や missing `HOME` に対するテストがない。これは小さいが public construction path である。

## 追加を提案するテスト

- Codex OAuth construction invariants に対する provider-factory tests を追加する:
  - `ref = "codex-oauth/gpt-5.5"` が `AuthRef::CodexOAuth` として resolve/build される。
  - `base_url` がない場合、official OpenAI ではなく ChatGPT backend base URL になる。
  - explicit `base_url` は引き続き優先される。
  - 広い internals を露出せずに assert できるなら、Codex OAuth OpenAI Responses が unsupported output/sampling parameters を無効化する。

- 対称的な model catalog override tests を追加する:
  - `load_models()` は `<config_dir>/models.toml` を優先する。
  - malformed model override は `CatalogError::Parse` を返す。
  - provider override と model override は片方だけが存在する場合も独立している。

- Codex OAuth race-guard tests を追加する。できれば `auth_json::load` 周辺の小さな test seam を使うか、呼び出し間で temp-file changes を構成する:
  - stale initial snapshot、fresh pre-refresh snapshot: HTTP refresh しない。
  - pre-refresh account id が異なる: HTTP refresh せず、新しい account を採用する。

- staleness fallback unit tests を追加する:
  - invalid/non-JWT access token + recent `last_refresh` は stale ではない。
  - invalid/non-JWT access token + old `last_refresh` は stale。
  - invalid/non-JWT access token + missing `last_refresh` は stale。

- `wiremock` で request body matching を使って refresh HTTP tests を強化し、偶発的な Codex OAuth request-shape 変更を捕捉できるようにする。

- catalog churn が想定されるなら、正確な builtin provider-count tests をより contract-focused な checks に置き換えることを検討する:
  - required provider ids が存在する
  - 各 provider が必要な scheme/auth/default context/capability を持つ
  - 各 builtin model が known provider を参照する
  
  正確なリストが意図的に product contract の一部であるなら、現在の snapshot-style assertions で問題ない。

## 実行したコマンド

- `cargo test -p provider`
  - Result: passed
  - Summary: `47 passed; 0 failed; 0 ignored`
  - Doc tests: `0 passed; 0 failed`

