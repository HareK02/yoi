# LLMのReasoningコンテキスト管理仕様 比較レポート

**対象**: Claude (Anthropic) / ChatGPT (OpenAI) / Ollama
**作成日**: 2026年5月4日
**目的**: 各LLMプロバイダがReasoning（思考トレース）をマルチターン会話でどのように扱うかを整理し、実装時の注意点をまとめる。

---

## 1. はじめに

Reasoning対応モデルは、最終応答の前に「思考プロセス」を生成する。この思考はユーザーに見せる/見せない、次ターンに残す/残す、ツール使用中に保持する/しない、といった扱いがプロバイダごとに大きく異なる。本レポートでは Claude / ChatGPT / Ollama の3プラットフォームについて、コンテキスト管理の仕様を比較する。

主要な論点は以下の3つ:

1. **思考の表現形式** — どのフィールド/ブロックに格納されるか
2. **マルチターン保持** — 次のユーザーターンに進んだとき思考は残るか
3. **ツール使用との関係** — ツール呼び出しの前後で思考をどう扱うか

---

## 2. Claude (Anthropic)

### 2.1 表現形式

Claudeは API レスポンスに専用の `thinking` ブロックを返す。コンテンツブロック配列の中に `type: "thinking"` の要素として並び、最終応答は `type: "text"` ブロックとして別に格納される。

`thinking` ブロックには `signature` という暗号化フィールドが付与され、改ざん検知や正当性確認に使われる。これは Claude 4 系列で長くなり、API 利用者は値を解釈・パースしてはならない仕様。

### 2.2 マルチターン保持の挙動

モデル世代によってデフォルトが分かれる:

- **Opus 4.5+ / Sonnet 4.6+**: 過去ターンの thinking ブロックがコンテキストに**保持される**（入力トークンとしてカウント）
- **それ以前の Opus/Sonnet および全 Haiku**: 過去ターンの thinking ブロックは**剥離される**（コンテキストに加算されない）

これは Anthropic 側の方針転換で、新世代では「推論の連続性」を優先する設計に変わった。実効コンテキストウィンドウの計算式は次のようになる:

```
context_window = (current input tokens − previous thinking tokens) + (thinking tokens + encrypted thinking tokens + text output tokens)
```

### 2.3 ツール使用との関係

ツール使用時は仕様が厳密になる。`tool_use` → `tool_result` → 続きのアシスタント応答という流れが**同一論理ターン**として扱われ、この間 thinking ブロックは**必ず保持して API に渡し戻す**必要がある。これはモデルの推論連続性を維持するため。

ただし境界条件として、tool_result ではない通常の user メッセージが入った時点で、それまでの thinking ブロックは無視・剥離される。つまり「新しいユーザー発話」がリセットポイントになる。

### 2.4 制御API

`clear_thinking_20251015` という context-editing 戦略により、開発者側で保持ポリシーを上書きできる:

```python
context_management={
    "edits": [{
        "type": "clear_thinking_20251015",
        "keep": {"type": "thinking_turns", "value": 2}
    }]
}
```

`keep.value` で「直近Nターン分の thinking だけ残す」といった粒度で指定可能。新世代モデルで thinking 保持がデフォルトになったため、コンテキスト圧迫を避けたい場合に使う。

---

## 3. ChatGPT (OpenAI)

### 3.1 表現形式

OpenAIの設計思想は Claude/Ollama と大きく異なり、**生のreasoningトークンはユーザーに見せない**。安全性上の理由から、reasoning は要約された形式（reasoning summaries）でのみ可視化される。

API レスポンスには `ResponseReasoningItem` という型のオブジェクトが含まれ、これは ID とオプションの暗号化コンテンツを持つが、本文は黒箱として扱われる。raw reasoning を `reasoning summary` 以外の方法で抽出しようとする試みは利用規約違反となる可能性がある。

### 3.2 マルチターン保持の挙動

ここが最も特殊。**Chat Completions API と Responses API で挙動が違う**。

#### Chat Completions API (旧来)

reasoning トークンは各ターンの後に破棄される。次ターンには引き継がれない（input/output tokens のみが次ステップに送られる）。これは o1 系列の初期設計。

#### Responses API (推奨)

ステートフル管理が可能。reasoning items を次のリクエストに引き継ぐには2方式ある:

1. `previous_response_id` パラメータで過去のレスポンスを参照
2. `response.output` の全アイテムを次の `input` に手動で渡す

ステートレス利用（`store=false`、ZDR組織）の場合は `include=["reasoning.encrypted_content"]` を指定すれば暗号化された推論コンテンツを受け取り、次リクエストに渡すことで推論を引き継げる。Yoi は履歴から復元した reasoning item を通常の API message として扱い、独自の turn-boundary filtering はしない。

同一ターン内の function-call loop でも、`reasoning item → function_call → function_call_output → 次の Responses request` の連続性を保つため、履歴上の reasoning item は通常の API message として保持する。ToolResult は wire 上で user 側 item に見えるが、reasoning item の削除境界としては扱わない。

#### モデル世代差

- **o1 / o3-mini / o1-mini / o1-preview**: フォローアップリクエストで reasoning items は常に無視される（input に含めても）
- **o3 / o4-mini 以降**: function call に隣接する一部の reasoning items はコンテキストに含まれ、ツール使用の連続性を改善する

ZDR (Zero Data Retention) を有効にした組織でも、暗号化機構により reasoning items を OpenAI 側に保存せずに API リクエスト間で再利用できるようになっている。

### 3.3 ツール使用との関係

o3 / o4-mini 以降では、function call をチェーン・オブ・ソート内で直接呼べるようになっており、reasoning tokens がリクエスト・ツール呼び出しを跨いで保持される。これにより複数ステップのエージェント的タスクでインテリジェンスが向上し、コスト/レイテンシも削減される（推論キャッシュの再利用）。

### 3.4 制御API

`reasoning.effort` パラメータで思考量を `low` / `medium` / `high` で指定可能（o1-mini は非対応）。`max_output_tokens` で reasoning + 最終出力の合計トークンを制限できる。

---

## 4. Ollama

### 4.1 表現形式

Ollamaはローカル実行プラットフォームで、モデルごとに思考タグの規約が異なる。レスポンスでは `message.thinking`（chat エンドポイント）または `thinking`（generate エンドポイント）に推論トレース、`message.content` / `response` に最終回答が分離されて格納される。

内部的にはモデル固有のパーサが動く:

- **Qwen3Parser** — Qwen 系の thinking タグとツール呼び出しタグを処理
- **DeepSeek3Parser** — DeepSeek の推論出力に最適化
- **Harmony** — GPT-OSS モデル用、low/medium/high の段階的 thinking レベルをサポート

ハードコードされたパーサがないモデルでは `thinking.InferTags` がプロンプトテンプレートをスキャンし、`<think>...</think>` などのデリミタを推測する。

### 4.2 マルチターン保持の挙動

**Ollama自体はthinking履歴を管理しない**。これは設計上の重要な特徴で、メッセージ配列を組み立てるクライアント側の責任になる。

モデル側のテンプレート設計レベルで「履歴に思考を残すな」と明示しているケースが多い。例えば Gemma 4 系は公式ドキュメントで明示的に「マルチターン会話では過去のモデル出力には最終応答のみを含めるべきで、過去のモデルターンの思考は次のユーザーターンの前に追加してはいけない」と指示している。DeepSeek-R1 や Qwen3 も同様の前提で訓練されている。

したがって標準的な実装パターンは「**次ターン送信時に thinking フィールドを落とし、content だけ履歴に積む**」となる。Ollama が thinking を別フィールドに分離しているのは、ユーザーが履歴構築時に簡単に捨てられるようにする意図もある。

### 4.3 ツール使用との関係

ツール呼び出しループで思考を保持したい場合、`clear_thinking=false` を上流APIで設定することで `<think>` ブロックを会話コンテキストに保持できる。OllamaはThinkValueが nil または true のときこれを自動処理する。

実用上は Open WebUI などのクライアント実装が参考になる。Open WebUI は同一ターン内のツール呼び出しでは reasoning コンテンツを保持し、`<think>...</think>` タグでシリアライズして次のAPIコール時の assistant メッセージの content フィールドに含める方式を採っている。

### 4.4 制御API

主要な制御は以下:

- `think` パラメータ — boolean（true/false）、ただし GPT-OSS は `"low"` / `"medium"` / `"high"` の文字列のみ受け付ける
- CLI: `--think` / `--think=false` / `--hidethinking`（思考はするが表示しない）
- サーバー起動時: `ollama serve --reasoning-parser deepseek_r1` でパーサを明示

サポート判定は `supportsThinking` 関数が `config.json` を見て、glm4moe / deepseek / qwen3 などの既知アーキテクチャか確認する仕組み。

---

## 5. 比較表

### 5.1 仕様サマリー

| 観点 | Claude | ChatGPT (OpenAI) | Ollama |
|---|---|---|---|
| **思考の可視性** | 完全可視（生のthinking） | 要約のみ可視（生は黒箱） | 完全可視（モデル次第でタグ形式） |
| **思考の格納先** | 専用ブロック (`type: "thinking"`) | `ResponseReasoningItem` | `message.thinking` フィールド |
| **暗号化/署名** | signature付き | encrypted_content（オプション） | なし |
| **デフォルトの履歴保持** | Opus 4.5+/Sonnet 4.6+: 保持<br>それ以前: 剥離 | Chat Completions: 破棄<br>Responses: 引き継ぎ可 | クライアント責任<br>（モデル指示は剥離が主流） |
| **ツール使用中の保持** | 必須（同一ターン内） | o3/o4-mini で関連部分自動保持 | `clear_thinking=false` で制御 |
| **管理層** | API側がマネージド | API側がマネージド | クライアント側で実装 |
| **思考量制御** | `clear_thinking_20251015`戦略 | `reasoning.effort` (low/medium/high) | `think`パラメータ (bool / level) |

### 5.2 設計思想の違い

| プロバイダ | 思想 |
|---|---|
| **Claude** | 「マネージドな推論ブロック」として抽象化。署名付きで改ざん耐性を持たせ、API側で保持/剥離を判断 |
| **ChatGPT** | 「思考は黒箱、要約だけ見せる」。安全性とIP保護優先で、推論の引き継ぎはAPI仕様（Responses）でハンドル |
| **Ollama** | 「プロンプトに混ぜるタグの開閉とパース」という素朴な仕組み。クライアントの自由度が高いが責任も大きい |

---

## 6. 実装上の注意点

### 6.1 共通の落とし穴

- **モデル世代差を見落とす** — Claude も OpenAI も、世代によってデフォルトが大きく異なる。バージョン固定や挙動確認は必須
- **ツール使用ループでの推論喪失** — Claude/Ollama 共に、ツール使用中に thinking を落とすと推論連続性が壊れる。同一ターン内は必ず保持する
- **トークンコスト** — Claude 新世代では thinking が入力トークンとして加算される。長い対話では `clear_thinking_20251015` でのトリミングが必要

### 6.2 プロバイダ別の推奨パターン

**Claude を使うとき**
- 4.5+/4.6+ ではデフォルトで thinking が残るため、長期会話ではコンテキスト圧迫に注意
- 旧世代との互換コードを書く場合は、両方のデフォルト挙動を意識する
- ツール使用時は受け取った thinking ブロックを**完全な形で**渡し戻す

**ChatGPT を使うとき**
- 新規実装は **Responses API** を選ぶ（Chat Completions は推論引き継ぎが弱い）
- ZDR組織でも `reasoning.encrypted_content` で推論を引き継げる。履歴上の reasoning item は通常の API message として扱い、独自の turn-boundary filtering はしない
- raw reasoning の抽出を試みない（規約違反の可能性）

**Ollama を使うとき**
- クライアント側で thinking フィールドの扱いを明示的に決める
- モデル毎のテンプレート規約を確認する（Gemma 4 のように「履歴に残すな」指示があるモデルもある）
- サーバー起動時に `--reasoning-parser` を適切に設定する

---

## 7. まとめ

3プラットフォームのReasoning管理は、それぞれ異なる優先事項を反映している:

- **Claude** は「推論の透明性と連続性」を最重視し、API側でリッチなマネジメントを提供
- **ChatGPT** は「安全性とIP保護」を最重視し、生の推論をユーザーから隠蔽しつつ機能性は Responses API で担保
- **Ollama** は「ローカル実行と柔軟性」を最重視し、クライアントに最大限の制御権を委ねる

実装者にとっては、「次のユーザーターンに進んだ時点で何が剥がれるか」を**プロバイダ × モデル世代 × API選択**の3軸で把握しておくことが重要。特にツール使用を伴うエージェント構築では、各プラットフォームの推論保持機構を正しく使わないと、推論連続性の喪失によりインテリジェンスが大幅に低下する。

---

## 参考資料

- Anthropic: [Building with extended thinking](https://docs.claude.com/en/docs/build-with-claude/extended-thinking)
- Anthropic: [Context editing](https://platform.claude.com/docs/en/build-with-claude/context-editing)
- OpenAI: [Reasoning models guide](https://developers.openai.com/api/docs/guides/reasoning)
- OpenAI Cookbook: [Better performance from reasoning models using the Responses API](https://cookbook.openai.com/examples/responses_api/reasoning_items)
- Ollama: [Thinking capability docs](https://docs.ollama.com/capabilities/thinking)
- Ollama Blog: [Thinking](https://ollama.com/blog/thinking)
