# Google Gemini API: `maxOutputTokens` パラメータ仕様

Source: https://ai.google.dev/api/generate-content  
Source (thinking): https://ai.google.dev/gemini-api/docs/thinking  
Source (Gemini 2.5 Flash): https://ai.google.dev/gemini-api/docs/models/gemini-2.5-flash  
Source (Gemini 2.5 Pro): https://ai.google.dev/gemini-api/docs/models/gemini-2.5-pro  
Retrieved: 2026-04-28

---

## 1. パラメータ名と位置

`generationConfig.maxOutputTokens`

リクエストボディのトップレベルではなく、`generationConfig` オブジェクト内に配置する。
SDK では `GenerateContentConfig(max_output_tokens=...)` として渡す。

## 2. 必須 / 任意

**任意 (optional)**。省略時はモデルのデフォルト上限が適用される。

## 3. 型と範囲

- 型: `integer`
- モデル別の最大値:
  - `gemini-2.5-flash`: 最大 **65,536** トークン
  - `gemini-2.5-pro`: 最大 **65,536** トークン
- 最小値の公式明記はないが、正の整数を指定する。

## 4. thinking トークンとの関係

- `maxOutputTokens` が制限するのは**最終レスポンスの出力トークン数のみ**。thinking トークンは `usageMetadata.thoughtsTokenCount` として別途計上され、`maxOutputTokens` のカウントには含まれない。
- thinking トークンの制御には `generationConfig.thinkingConfig.thinkingBudget` を用いる。
  - Gemini 2.5 Flash / Pro: `128`〜`32768` トークン、`0` で thinking 無効化（モデルによる）、`-1` で動的
- 課金は「output tokens + thinking tokens」の合算。
- `maxOutputTokens` と `thinkingBudget` は独立したパラメータであり、両方を同時に指定できる。

> **注意**: 2025年10月時点で `gemini-2.5-flash` において `max_output_tokens` が無視されるバグが報告されており、Google 側が修正をロールアウトした経緯がある。最新モデルで想定通りに機能するか実測で確認することを推奨。

## 5. ドキュメント URL

- API リファレンス (GenerationConfig): https://ai.google.dev/api/generate-content#v1beta.GenerationConfig
- Thinking ガイド: https://ai.google.dev/gemini-api/docs/thinking
- Gemini 2.5 Flash モデル仕様: https://ai.google.dev/gemini-api/docs/models/gemini-2.5-flash
- Gemini 2.5 Pro モデル仕様: https://ai.google.dev/gemini-api/docs/models/gemini-2.5-pro
