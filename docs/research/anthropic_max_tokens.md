# Anthropic Messages API: `max_tokens` パラメータ仕様

Source: https://platform.claude.com/docs/en/api/messages  
Retrieved: 2026-04-28

---

## 1. パラメータ名

`max_tokens`

`max_output_tokens` ではない。

## 2. 必須か任意か

**必須 (required)**。

`POST /v1/messages` のボディパラメータとして必須指定。  
insomnia の現在の実装（`max_tokens: u32`、未指定時 4096 にフォールバック）は仕様と合致している。

## 3. 型・範囲

- 型: integer (number)
- 意味: 生成を停止する前に出力できるトークンの上限。モデルはこの値に達する前に停止することもある（上限の指定であり、保証値ではない）
- モデルごとに最大値が異なる:
  - Claude Opus 4.6 / 4.7: 最大 128k トークン
  - Claude Sonnet 4.6 / Haiku 4.5: 最大 64k トークン
  - Message Batches API + beta ヘッダ `output-300k-2026-03-24`: 最大 300k トークン（Opus 4.7, 4.6, Sonnet 4.6）

## 4. Extended Thinking との組み合わせ制約

Source: https://platform.claude.com/docs/en/build-with-claude/extended-thinking

- `thinking.budget_tokens` は必ず `max_tokens` **未満** でなければならない
- thinking トークンは `max_tokens` の上限に含まれてカウントされる
- `budget_tokens` の最小値は **1,024 トークン**
- 例外: ツールを伴う interleaved thinking では `budget_tokens` が `max_tokens` を超えることが許容される（予算がコンテキストウィンドウ全体に対して適用されるため）
- `max_tokens` が 21,333 を超える場合はストリーミングが必須
- Claude Opus 4.6 / Sonnet 4.6 以降では `budget_tokens` は非推奨になり、代わりに `effort` パラメータによる adaptive thinking が推奨されている

## 5. ドキュメント URL

- Messages API リファレンス: https://platform.claude.com/docs/en/api/messages
- Extended Thinking ガイド: https://platform.claude.com/docs/en/build-with-claude/extended-thinking
