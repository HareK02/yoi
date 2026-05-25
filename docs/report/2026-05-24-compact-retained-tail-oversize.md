# Compact 後 retained tail が巨大化して次 turn が壊れた疑い

## 概要

2026-05-24 の開発セッション中、手動 compact 後に「続けられる？」と入力した turn が、ユーザー側では context length 超過系のエラーで切れたように見えた。セッション永続化を確認すると、manual compact 自体は成功扱いで新 segment を作っているが、その post-compact `SegmentStart.history` が 892 entries / 約 2.3MB と巨大なままだった。

その直後の turn は `invoke` / `user_input` / `turn_end` / `run_completed result=finished` が残っている一方で、`assistant_item` と `llm_usage` が残っていない。つまり Pod 永続化上は正常終了扱いだが、実際には LLM 応答が生成されていない空 turn になっている。

次の turn の前には自動 pre-run compact がもう一度走り、今度は `SegmentStart.history` が 24 entries まで縮んだ。その後の turn は正常に `llm_usage input_total_tokens=20182` を記録して進行している。

## 観測したセッション

親 session:

- `<insomnia-sessions>/019e5769-73fa-72a0-b501-b657a8976dd3`

関連 segment:

- 元 segment: `019e5769-73fa-72a0-b501-b665cb0ce470`
- 1回目 compact 後: `019e5c2f-a23c-7a00-b5db-fb31fccec9fb`
- 2回目 compact 後: `019e5c34-cea7-7e72-8565-2d5447fa0b70`

1回目 compact 後の `SegmentStart.history` は 892 entries。内訳は概ね以下。

- `tool_call`: 338
- `tool_result`: 338
- `reasoning`: 171
- `message`: 45

これは 1 turn で大量 tool call したというより、前回 compact 以後の長い履歴 suffix が retained tail として残ったものと考える方が自然。

## 現時点の推定

### 1. manual compact は「400K 未満にする」処理ではない

400K は compact 発火閾値であり、compact 後の post-condition ではない。manual compact の履歴分割は `compact_retained_tokens` を目標に末尾履歴を残すが、その結果が compact 閾値未満かどうかを強く検証していないように見える。

### 2. retained tail の見積もりが実際の persisted history サイズと乖離している可能性

`split_for_retained` / `token_estimates_for_prune_impl` 周辺を見る限り、retained split は usage records や token estimate に依存する。LLM request 時には tool result pruning / projection が関わるため、LLM に実際に投げた context の usage と、session log に永続化されている unpruned history の大きさが一致しない可能性がある。

その場合、manual compact 前の retained split では「小さく見える」が、compact 後に usage records がない状態で byte fallback 的な推定を使うと「大きく見える」ため、直後に auto compact が必要になる、という挙動を説明できる。

この点はまだ確定ではない。次に `019e5c2f` の retained tail が旧 history のどの index から始まったか、当時の usage records がどの history_len に対応していたかを再現コードか追加ログで詰める必要がある。

### 3. `just_compacted` が safety net を一時的に止める

compact 成功後は `compact_state.record_compact_success()` により `just_compacted = true` になる。`prepare_for_run()` の pre-run compact と interceptor の between-request compact は `!just_compacted` を条件にしているため、compact 直後の次 turn では再 compact が抑止される。

今回、1回目 compact の post-compact history が巨大なままでも、直後の turn ではその safety net が効かなかった。その turn が実質失敗しているのに `run_completed result=finished` として扱われたことで `just_compacted` が解除され、さらに次の turn の pre-run compact が走ったと考えられる。

### 4. context length 系失敗が `run_errored` として残っていない

問題の turn には `assistant_item` / `llm_usage` が無いにもかかわらず `run_completed finished` が残っている。ユーザー体感では context length 超過で切れている。LLM worker 側、stream continuation 側、または rollback/empty turn 処理で、エラーが正常終了扱いに丸められている可能性がある。

これは compact 問題とは別に、永続化と UI observability のバグとして調べるべき。

## 追加で気になった点

runtime state に segment 表示の不一致があった。

- `<runtime-dir>/insomnia/status.json` は古い segment id を指していた
- `<runtime-dir>/pods.json` は新しい segment id を指していた

今回の context 超過の主因ではなさそうだが、attach/restore 時に混乱要因になり得る。

## 修正候補

1. compact 成功後に post-compact context estimate を検査する。
   - `new_history_len`
   - estimated prompt tokens
   - retained item count
   - retained byte size
   - threshold に対する比率

2. post-compact history が threshold を超える場合、単純な成功扱いにしない。
   - `CompactFailed` 相当にする
   - あるいは `just_compacted` を立てず、pre-run safety net を有効のままにする
   - ただし infinite compact loop / thrash を避けるため、失敗理由を明示する必要がある

3. retained split の token estimate を、LLM に投げた pruned context ではなく、persisted history の実サイズに近い形で検証する。
   - 少なくとも post-condition は usage record だけに依存しない
   - byte fallback と usage-based estimate の乖離を metrics に出す

4. tool call / tool result boundary 保護が retained budget をどれだけ破ったかを可視化する。
   - `initial_cut`
   - `balanced_cut`
   - `items_pulled_back`
   - `bytes_pulled_back`
   - `estimated_tokens_pulled_back`

5. compact metrics を session log に残す。
   - source: manual / pre_run / between_requests
   - old_segment_id / new_segment_id
   - old_history_len / new_history_len
   - retained_from index
   - retained_items
   - estimated_tokens_before / after
   - estimate source: usage_records / byte_fallback / mixed

6. context length 系エラーが `run_completed finished` になる経路を調べる。
   - `assistant_item` と `llm_usage` が無い run を正常終了扱いにしてよいか
   - LLM request 前の context-build error と upstream error の永続化
   - TUI に `RunEnd(Finished)` だけが届く経路がないか

## 次にやる調査

- `019e5c2f` の 892-entry history が、旧 segment history の何番目から retained されたものか特定する。
- 旧 segment の `LogEntry::LlmUsage { history_len, input_total_tokens, ... }` と retained cut の対応を確認する。
- `split_for_retained` を当時の history / usage records に対して再実行し、見積もりと実 persisted size の差を出す。
- `llm-worker` の prune threshold / protected area / min savings を確認し、pruning がこの乖離にどの程度寄与したかを確定する。
- 空 turn が `run_completed finished` になった理由を追う。

## 注意

このレポートは調査途中の暫定まとめ。特に「pruning が retained estimate を小さく見せた」という仮説はまだ未確定。確実に言えるのは、manual compact 後の `SegmentStart.history` が 892 entries と巨大で、その直後の turn が assistant/usage 無しに finished 扱いになり、次 turn で再 compact されて復旧した、という観測事実。
