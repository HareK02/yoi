# LLM client: OpenAI Responses の incomplete / unknown field 観測性

## 背景

CodexOAuth / OpenAI Responses 経路で、Prune 後の大きな request に対して `response.incomplete` が SSE event として返り、出力が中断される症状が観測された。

旧 OpenAI Responses parser は `response.failed` / `response.incomplete` を既知 field だけに deserialize し、`response.error` がある場合だけ `ErrorEvent` に反映していた。`response.error` が無い `response.incomplete` は `response response.incomplete` という汎用 message に潰れるため、provider が返した `incomplete_details` や response 直下の未知 field を後から確認できなかった。

Responses API は event schema が増える可能性がある。既知 field だけを構造体に deserialize して未知 field を暗黙に捨てると、障害調査時に「provider は何を返していたか」が失われる。特に incomplete / failed / top-level error は、復旧判断や retry 対象判定に使う可能性があるため、未知 field を観測可能な形で残す必要がある。

## 実装結果

対象コミット:

```text
32ae63d fix: preserve openai responses incomplete diagnostics
```

実装内容:

- `response.failed` / `response.incomplete` を parse する際、既知 field だけでなく `serde(flatten)` で unknown / extra field を保持する。
- `response.incomplete_details.reason` を diagnostic に残す。
  - `response.error` が無い `response.incomplete` でも `OpenAI Responses response.incomplete` と diagnostic を含む message にする。
  - reason がある場合は `ErrorEvent.code` にも入る。
- `response.failed` / `response.incomplete` の response 直下 extra field、error object 内 extra field、event top-level extra field を diagnostic に残す。
- top-level `error` event についても、error object 内 extra field と event top-level extra field を diagnostic に残す。
- diagnostic は `ErrorEvent.message` に付与する。
  - session history / LLM context には混ぜない。
  - raw frame 全体の恒久保存ではなく、extra field を size cap 付き diagnostic として残す。
- diagnostic value は JSON 文字列化後 512 chars で cap する。

追加 test:

- `response.incomplete` に `incomplete_details.reason` が含まれる場合、reason が `ErrorEvent` に残る。
- `response.error` が無い `response.incomplete` でも汎用 message だけにならない。
- `response.incomplete` の response 直下 unknown field が diagnostic に残る。
- `response.failed` の response/error unknown field が diagnostic に残る。
- top-level `error` event の unknown field が diagnostic に残る。

確認済み:

```text
cargo test -p llm-worker openai_responses --lib
cargo test -p llm-worker --lib
cargo fmt --check
```

## 完了判定

incomplete / failed / top-level error の diagnostic 観測性は実装済み。自動 retry、Prune 閾値調整、prompt cache key / previous_response_id 設計変更、raw SSE frame 全体の恒久保存は範囲外として残す。

## 範囲外

- `response.incomplete` の自動 retry 実装。
- Prune 閾値 / candidate 選択の変更。
- prompt cache key / previous_response_id の設計変更。
- raw SSE frame 全体の恒久保存を常時有効にすること。
