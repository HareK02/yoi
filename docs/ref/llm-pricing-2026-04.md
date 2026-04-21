# LLM 料金サマリ

調査日: 2026-04-19。料金は頻繁に改定されるため一次ソースで再確認。

## 定額サブスク

| サービス | 月額 | 含まれる上限 | 実用性メモ |
|---|---|---|---|
| Claude Pro | $20 | Sonnet 中心、5h rolling + 週次 | Opus 4.7 はほぼ使えない |
| Claude Max 5x | $100 | Pro×5、Sonnet 常用可 | Sonnet をガッツリ書くなら最低ライン |
| Claude Max 20x | $200 | Pro×20、Opus 4.7 解禁 | Opus をエージェント用途で回すならここ |
| ChatGPT Plus | $20 | Codex 5h で GPT-5.4 が 20–100 msg | 長時間 agent loop では枯渇 |
| ChatGPT Pro | $100 | Codex 5h で 100–500 msg、**2026-05-31 まで 2x プロモ**で 600–3000 | Max 20x 相当の常用級 |
| Copilot Free | $0 | 50 premium/月、Haiku 4.5 / GPT-5 mini | お試し |
| Copilot Pro | $10 | 300 premium/月、GPT-5 mini/4.1/4o は乗数 0x（無制限扱い） | コスパ◎ |
| Copilot Pro+ | $39 | 1,500 premium/月、Opus 4.7 含む全モデル | 定額で Opus 4.7 最安 |
| Cursor Pro | $20 | $20 クレジット、fast 500/月、slow 無制限 | 2025/6 に usage-based へ移行 |
| Cursor Pro+ | $60 | Pro×3 クレジット | 詳細は公式ダッシュボード |
| Cursor Ultra | $200 | Pro×20 クレジット、Privacy Mode | Max 20x 競合 |
| Zed Pro | $10 | トークン課金、BYOK 可 | 実質「安い BYOK フロント」 |
| Windsurf Pro | $20（2026/3 値上げ） | クレジット制、Claude 系のみ BYOK 可 | Teams/Ent は BYOK 不可 |
| Ollama Pro | $20（年 $200） | 5h + 7d、Free×50、3 並列 | `:cloud` モデル (`gpt-oss:120b`, `deepseek-v3.1:671b` 等) |
| Ollama Max | $100 | Pro×5、10 並列 | ローカル + クラウド併用 |
| BLACKBOX Pro/Plus/Max | $10 / $20 / $40 | FUP 非公開 | Unlimited の実態不明 |
| xAI SuperGrok | $30 | Grok 4 + 2M ctx（Web/アプリ） | **API 枠なし** |
| xAI SuperGrok Heavy | $300 | Grok 4 Heavy 独占（Web/アプリ） | **API 枠なし** |
| X Premium+ | $40 | Grok Web 利用 | **API 枠なし** |

## 無料枠・従量（少額）

- **OpenRouter**: プリペイド方式、手数料 5.5%（暗号通貨 5%）。**BYOK は月 100 万 req 無料**、超過後は元コストの 5%。`:free` モデルは 50 req/日、$10 以上入金で 1000 req/日
- **Google AI Studio (Gemini 2.5 Pro)**: 5 RPM / **100 RPD** / TPM 250k 共有。2025/12 に大幅減枠
- **GitHub Models (GPT-4o 等)**: 10 RPM / 50 RPD / 8k in, 4k out / 2 並列
- **Cerebras Free**: 30 RPM / **1M tokens/日**（Llama 3.3 70B, Qwen3, gpt-oss-120B）
- **Groq Free**: サインアップのみ（CC 不要）。Llama 3.3 70B は 30 RPM / 12k TPM / 1k RPD / 100k TPD、モデル別に上限異なる。**Developer tier**（要 CC）で Free の約 10× + Batch/Flex 解放 + daily cap 撤廃。月額プランは Enterprise のみ
- **DeepSeek API**: V3 $0.14/$0.28, R1 $0.55/$2.19 /1M tok。オフピーク 50–75% 割引、キャッシュ 90% 割引
- **Moonshot Kimi**: Adagio 無料プランあり。K2.5 API は $0.60/$2.50 /1M tok
- **Together / Fireworks / DeepInfra**: サインアップクレジットのみ、恒久無料枠なし

## 従量 API の単価比較（コーディング向け）

| プロバイダ | モデル | 入/出 ($/1M tok) | Ctx | 備考 |
|---|---|---|---|---|
| Anthropic | Haiku 4.5 | $1.00 / $5.00 | 200k | 軽量・高速 |
| Anthropic | Sonnet 4.6 | $3.00 / $15.00 | 200k | 精度上位級 |
| Anthropic | Opus 4.7 | $5.00 / $25.00 | 200k | エージェント最強（旧 $15/$75 から値下げ） |
| xAI | grok-code-fast-1 | $0.20 / $1.50 | 256k | SWE-bench 70.8%、142 tok/s |
| xAI | grok-4-1-fast | $0.20 / $0.50 | 2M | 長文向け |
| xAI | grok-4 | $3.00 / $15.00 | 256k | 旗艦（旧） |
| Groq | GPT-OSS 120B | $0.15 / $0.60 | 128k | ~500 tok/s（LPU） |
| Groq | Kimi K2-0905 | $1.00 / $3.00 | 256k | open モデル最強級 |
| Groq | Llama 3.3 70B | $0.59 / $0.79 | 131k | 汎用 |
| Groq | Qwen3 32B | $0.29 / $0.59 | 131k | 汎用 |
| DeepSeek | V3 | $0.14 / $0.28 | 128k | オフピーク 50–75% 引、cache 90% 引 |
| DeepSeek | R1 | $0.55 / $2.19 | 128k | 推論系最安 |
| Moonshot | Kimi K2.5 API | $0.60 / $2.50 | — | 無料 Adagio プランあり |

プロンプトキャッシュ: Anthropic / xAI / Groq すべて対応（50–90% 引）。長い system prompt を使うエージェントでは実効単価が大きく下がる。

## 従量回避派の推奨構成

### A. Claude Max 単騎 ($100)
Sonnet 4.6 中心で Claude Code + Web を併用。物足りなければ Max 20x ($200) に上げて Opus 4.7 を解禁。

### B. Copilot Pro+ + 無料枠併用 ($39)
1,500 premium/月 + GPT-5 mini/4.1/4o が乗数 0x 無制限。溢れを Gemini 2.5 Pro (100 RPD) / Cerebras (1M tok/日) に逃がす。Opus 4.7 は乗数 7.5x = 実質 200 回/月なので温存必須。

### C. OpenRouter 少額 + Ollama Cloud + 無料枠 ($20–30)
OpenRouter に $10 入金で `:free` が 1000 req/日に拡張。Ollama Pro ($20) で `gpt-oss:120b` / `deepseek-v3.1:671b`。補助に GitHub Models / Groq / Cerebras。BYOK 持ちなら月 100 万 req 無料の BYOK トンネルも。

### D. ChatGPT Pro Codex プロモ ($100、~2026-05-31)
Codex 2x プロモで Max 20x 相当の実行量。プロモ終了後（6月以降）は通常枠 100–500/5h に戻る点は要確認。

### E. ハイブリッド最安 ($30)
Zed Pro ($10) + Ollama Pro ($20) + 無料枠フル動員（Gemini / Cerebras / GitHub Models / Groq）。Zed は Sonnet をトークン課金、高頻度タスクはローカル/Ollama Cloud へ流す。

## 注記（要確認）

- Claude Code の週次上限の具体値: Anthropic 公式ヘルプに正確な数値記載なし。Max 20x で Opus 4.7 が「概ね 24–40 時間/週」との二次情報あり
- Codex Pro 5x/20x の詳細と プロモ終了後の挙動
- BLACKBOX Unlimited の実スループット（公式非開示）
- Cursor Pro+ / Ultra のクレジット実効回数（モデル API コスト依存で一律換算不能）

## 一次ソース

- https://claude.com/pricing
- https://developers.openai.com/codex/pricing
- https://github.com/features/copilot/plans
- https://docs.github.com/en/copilot/concepts/billing/copilot-requests
- https://ollama.com/pricing
- https://openrouter.ai/pricing
- https://openrouter.ai/announcements/1-million-free-byok-requests-per-month
- https://ai.google.dev/gemini-api/docs/rate-limits
- https://docs.github.com/github-models/prototyping-with-ai-models
- https://console.groq.com/docs/rate-limits
- https://api-docs.deepseek.com/quick_start/pricing
- https://docs.x.ai/developers/models
- https://grok.com/plans
- https://groq.com/pricing
- https://console.groq.com/docs/rate-limits
