# LLM client: OpenAI Responses の incomplete / unknown field 観測性

## 背景

CodexOAuth / OpenAI Responses 経路で、Prune 後の大きな request に対して `response.incomplete` が SSE event として返り、出力が中断される症状が観測された。

現行の OpenAI Responses parser は `response.failed` / `response.incomplete` を `ResponseFailed` として deserialize し、`response.error` がある場合だけ `ErrorEvent` に反映している。`response.error` が無い `response.incomplete` は `response response.incomplete` という汎用 message に潰れるため、provider が返した `incomplete_details` や response 直下の未知 field を後から確認できない。

また、Responses API は event schema が増える可能性がある。既知 field だけを構造体に deserialize して未知 field を暗黙に捨てると、障害調査時に「provider は何を返していたか」が失われる。特に incomplete / failed / top-level error は、復旧判断や retry 対象判定に使う可能性があるため、未知 field を観測可能な形で残す必要がある。

## 要件

- `response.incomplete` の詳細 reason を失わない。
  - `response.incomplete_details.reason` がある場合は ErrorEvent / trace / log から確認できる。
  - `response.error` が無い場合でも `response response.incomplete` だけに潰さない。
- `response.failed` / `response.incomplete` で、既知 schema に入らない response field を観測できる。
  - まずは raw `serde_json::Value` から diagnostic を組み立てる形でよい。
  - 既知 field に昇格していない field があっても deserialize 時点で完全には捨てない。
- unknown / extra field の記録は、通常の session history には混ぜない。
  - LLM context に暗黙注入しない。
  - debug trace / structured log / error diagnostic のいずれか、調査用の経路に残す。
- 記録する raw JSON は肥大化と secret 混入に注意する。
  - full raw frame を常時 transcript に保存しない。
  - trace に残す場合は debug mode / size cap / redaction 方針を明確にする。
- OpenAI Responses の top-level `error` event も同じ方針で確認する。
  - `message`, `type`, `code` 以外の field がある場合に捨て切らない。
- incomplete / failed の diagnostic は、将来の retry 判定に使える粒度にする。
  - 例: `incomplete_reason`, provider error code/type, status 相当、raw extra keys。

## 完了条件

- `response.incomplete` に `incomplete_details.reason` が含まれる fixture で、ErrorEvent または trace/log diagnostic に reason が残る test がある。
- `response.incomplete` に `response.error` が無い fixture でも、汎用 message だけにならない test がある。
- `response.failed` / `response.incomplete` の response 直下 unknown field が観測用 diagnostic に残る test がある。
- top-level `error` event の unknown field について、捨て切らない方針が実装または test で確認されている。
- diagnostic が session history / LLM context に混入しないことが確認されている。
- `cargo fmt --check` と関連 crate の test が通る。

## 範囲外

- `response.incomplete` の自動 retry 実装。
- Prune 閾値 / candidate 選択の変更。
- prompt cache key / previous_response_id の設計変更。
- raw SSE frame 全体の恒久保存を常時有効にすること。
