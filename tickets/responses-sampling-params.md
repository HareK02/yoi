# OpenAI Responses: sampling パラメータの取り扱い

## 背景

ChatGPT backend (`https://chatgpt.com/backend-api/codex/responses`) は公式
OpenAI Responses API のサブセットしか受け付けず、サポート外パラメータを
含むリクエストを 400 (`Unsupported parameter: ...`) で拒否する。
受理パラメータは概ね以下に限られる（`docs/research/openai_responses_max_output_tokens.md`）:

```
model, input, instructions, stream, store, include,
tools, tool_choice, reasoning, previous_response_id, truncation
```

`max_output_tokens` については先行修正 (commit `af57d5b`) で
`OpenAIResponsesScheme::send_max_output_tokens` を導入し、
`AuthRef::CodexOAuth` 経路では送らないようにしてある。

今回、同じ経路で `temperature` も 400 を返すことが確認された:

```
[notice] pod: memory Phase 1 extract failed:
Client error: API error (status: 400):
{"detail":"Unsupported parameter: temperature"}
```

加えて、Pod の compactor / extract worker は `pod.rs` で
`.temperature(0.0)` をハードコードしている。「決定論的に振る舞う」程度の
動機で 0.0 が選ばれているが:

- 公式 reasoning モデル (`gpt-5`, o 系) は temperature を無視/固定する
- 他プロバイダ (Claude / Gemini / Ollama) でも 0.0 が extract / 要約に
  最適という自前検証は無い
- そもそもプロバイダ既定値がそれぞれの妥当な値になっているはず

ハードコードを残す積極的理由が弱く、かつ codex-oauth で実害が出ている。

## 方針

二段で対処する。

1. **wire-level**: `OpenAIResponsesScheme` に
   `send_sampling_params: bool` を追加し、`AuthRef::CodexOAuth` 経路では
   `false` に設定する。`false` のとき `temperature` / `top_p` を
   body に載せない。`max_tokens` と同じ枠組みなので構造は揃える。
2. **pod-level**: `pod.rs` の `.temperature(0.0)` ハードコード 2 箇所を
   撤去する。プロバイダ既定値に任せる。

(2) だけでも codex-oauth の現症状は消えるが、ユーザが manifest で
明示的に `temperature` を設定しているケース（非 0.0）でも codex-oauth
配下では 400 になるため、(1) も併せて入れる。

## 要件

### Scheme 側

- `OpenAIResponsesScheme` に `send_sampling_params: bool` フィールドを
  追加（デフォルト `true` = 公式 OpenAI API 向け）
- `with_send_sampling_params(bool)` ビルダを生やす
- `request.rs` の `ResponsesRequest` で `temperature` / `top_p` を
  `send_sampling_params == false` のときは `None` のまま送る
  （`#[serde(skip_serializing_if = "Option::is_none")]` で除外）
- `validate_config` で `send_sampling_params == false` かつ
  `config.temperature.is_some()` または `config.top_p.is_some()` の
  ときに `ConfigWarning::unsupported` を返す（`max_tokens` と同じ流儀）
- `provider/src/lib.rs` の `SchemeKind::OpenaiResponses` 分岐で、
  `AuthRef::CodexOAuth` のとき `send_sampling_params=false` を渡す

### Pod 側

- `crates/pod/src/pod.rs:1011` の compactor worker `.temperature(0.0)` を撤去
- `crates/pod/src/pod.rs:1368` の extract worker `.temperature(0.0)` を撤去
- 既存テストが落ちないことを確認（`pod.rs:2034` のテスト assert は
  `RequestConfig` に直接 `temperature: Some(0.2)` を入れているので
  ハードコード撤去とは独立）

### docs

- `docs/research/openai_responses_max_output_tokens.md` の
  「ChatGPT backend が拒否するパラメータ一覧」を補足するか、
  もしくは sampling 用の研究 doc を新設して `temperature` / `top_p`
  の扱いを明文化する（max_output_tokens の doc に追記する形で十分）

## 完了条件

- `OpenAIResponsesScheme::new().with_send_sampling_params(false)` で
  作った scheme から生成した body に `temperature` / `top_p` キーが
  載らない（unit test）
- `provider::build_client` で `AuthRef::CodexOAuth` + `OpenaiResponses`
  の組合せから作った client が `temperature` を含まないリクエストを送る
- pod の compaction / memory extract が codex-oauth 経由で 400 にならず
  最後まで走る
- `pod.rs` から `.temperature(0.0)` のハードコードが消えている
- `cargo check` / `cargo test` が `llm-worker`, `provider`, `pod` で通る

## 範囲外

- `user` / `metadata` 等、現状コードで送出していない他の拒否パラメータ
- 公式 OpenAI Responses API 側の `temperature` 挙動の変更
- 「extract / 要約タスクに最適な temperature は何か」という検証
  （必要になったら manifest で per-model 設定に逃がすのが筋であり、
  pod.rs 内に再ハードコードはしない）
