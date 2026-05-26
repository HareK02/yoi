# Compact: retained split が pruning 後 usage record に引きずられて巨大 tail を残す

## 背景

2026-05-26、起動中の `insomnia` Pod で compact 後の継続 turn が実質失敗した。compact 自体は summary を生成して新 segment を作成しており、処理としては成功扱いだった。しかし post-compact `SegmentStart.history` が 1957 items / 約 3.7MB と巨大なまま残り、その後の新 segment 上の turn は assistant output / `llm_usage` を残さず `run_completed finished` になった。

関連する過去事例として `docs/report/2026-05-24-compact-retained-tail-oversize.md` がある。今回の調査では、前回疑っていた「retained split が request-time pruning / projection 後の usage record と persisted history の実サイズのズレに引きずられる」問題が、より具体的に再現した。

対象観測:

```text
session: 019e5d30-f7ad-7fb1-bdec-c592e888e290
old segment: 019e5d30-f7ad-7fb1-bdec-c5a41394e6b1
new segment: 019e62d3-5cbf-7020-b696-8d661b5e1026
```

pre-compact history は 2192 items / 約 4.5MB。compact 後の `SegmentStart.history` は 1957 items / 約 3.7MB だった。

## 原因

`llm_usage.input_total_tokens` は「その LLM request に実際に投げた prompt の占有量」としては正しいが、旧 `split_for_retained` はそれを persisted/raw history の prefix token 数のように扱っていた。

旧 retained split は概ね以下の前提で動いていた。

```text
current = tokens_at(history.len())
target = current - retained_tokens
先頭から tokens_at(idx) >= target になる最初の idx を cut にする
```

この探索は `tokens_at(idx)` が prefix 長に対して単調増加することを前提にしている。しかし実際の usage record は pruning / projection 後の request 実測であり、raw persisted history の累積 token 数ではないため、大きく上下する。

今回の直接原因になった周辺 record:

```text
history_len=237 input_total_tokens=93358
history_len=240 input_total_tokens=20532
history_len=242 input_total_tokens=166263  # target を超えて cut に選ばれた
history_len=245 input_total_tokens=27680
history_len=247 input_total_tokens=85797
```

compact 直前の推定は以下だった。

```text
current estimate: 173310 tokens
retained target: 8000 tokens
split target: 165310 tokens
initial cut: 242
balanced cut: 242
retained items: 1950
```

`balance_to_pair_boundary` による引き戻しは今回発生しておらず、問題は tool call/result boundary 保護ではない。最初から usage record の外れ値によって cut が早すぎる位置に決まっていた。

## 実装結果

対象コミット:

```text
59de285 fix: compact retained split uses raw tail size
```

`crates/pod/src/compact/token_counter.rs` の retained split を、usage record の prefix crossing から raw serialized suffix size ベースに変更した。

- `llm_usage.input_total_tokens` は request occupancy として維持する。
- Compact の retained split では、per-`history_len` usage record を raw persisted history prefix の単調系列として使わない。
- current prompt occupancy を raw serialized bytes に配分し、末尾 persisted tail の byte size で cut を決める。
- `byte/4` fallback を下限に入れ、pruning 済み request measurement が低すぎても MB 級 raw history を retained 扱いしにくくした。
- `balance_to_pair_boundary` は raw suffix size で決めた cut に対して従来通り適用する。

追加した regression test:

- current occupancy を raw byte rate として使うこと。
- 非 current measurement を cut boundary として使わないこと。
- 非単調 usage spike があっても retained tail が巨大化しないこと。

## 完了判定

確認済み:

```text
cargo test -p pod compact::token_counter --lib
cargo test -p pod --lib
cargo fmt --check
```

このチケットでは、観測された直接原因である「非単調 usage record の外れ値によって retained cut が早すぎる位置に決まる」問題を修正した。

当初メモしていた post-compact context 検証、compact metrics 永続化、assistant output / `llm_usage` 無し turn の `finished` 扱いは、今回の retained split 修正とは別の safety/observability 課題であり、この修正の完了条件からは外す。

## 範囲外

- ローカルトークナイザの導入。
- 既存 session log の migration。
- compact summary prompt の大幅な再設計。
- TUI 表示だけで問題を隠す対応。

## 参考

- `docs/report/2026-05-24-compact-retained-tail-oversize.md`
- `crates/pod/src/compact/token_counter.rs`
- `crates/llm-worker/src/token_counter.rs`
