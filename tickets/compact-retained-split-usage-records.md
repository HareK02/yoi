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

## 原因の見立て

`llm_usage.input_total_tokens` は「その LLM request に実際に投げた prompt の占有量」としては正しいが、`split_for_retained` はそれを persisted/raw history の prefix token 数のように扱っている。

現在の retained split は概ね以下の前提で動く。

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

`balance_to_pair_boundary` による引き戻しは今回発生しておらず、問題は tool call/result boundary 保護ではない。最初から usage record の外れ値によって cut が早すぎる位置に決まっている。

## 要件

- Compact の retained split が request-time pruning / projection 後の `llm_usage.input_total_tokens` を raw persisted history prefix として誤用しない。
  - `llm_usage.input_total_tokens` は request occupancy としては維持してよい。
  - retained split 用の推定は、persisted/raw history の実サイズに近い基準で行う。
  - 少なくとも usage record が非単調な場合に、古い外れ値で cut が極端に早くならない。
- Compact 成功後に post-compact context の検証を行う。
  - retained item count
  - retained byte size / serialized history size
  - post-compact estimated tokens
  - threshold / retained budget に対する比率
- post-compact history が明らかに過大な場合、単純な成功扱いにしない。
  - `CompactFailed` 相当として扱う、または `just_compacted` を立てず次の safety net を有効にする。
  - infinite compact loop / thrash を避けるため、失敗理由は明示する。
- compact 後の次 turn が assistant output / `llm_usage` 無しに `run_completed finished` になる経路を潰す。
  - context length / request build / upstream error が発生した場合は `run_errored` または rollback として観測できる。
  - 少なくとも空 finished turn を正常完了として扱わない。
- compact metrics を session log か metrics extension で追えるようにする。
  - source: manual / pre_run / between_requests
  - old_segment_id / new_segment_id
  - old_history_len / new_history_len
  - retained_from index
  - retained_items / retained bytes
  - estimate source / post-condition result

## 完了条件

- 非単調な usage record が存在しても、retained split が巨大 tail を残さないことを確認する test がある。
- pruning / projection 後 usage と raw persisted history size が乖離するケースの regression test がある。
- post-compact context 検証が実装され、過大 tail の成功扱いを防ぐ test がある。
- compact 後の空 turn が `run_completed finished` にならないことを確認する test がある。
- `cargo fmt --check` と関連 crate の test が通る。

## 範囲外

- ローカルトークナイザの導入。
- 既存 session log の migration。
- compact summary prompt の大幅な再設計。
- TUI 表示だけで問題を隠す対応。

## 参考

- `docs/report/2026-05-24-compact-retained-tail-oversize.md`
- `crates/pod/src/compact/token_counter.rs`
- `crates/llm-worker/src/token_counter.rs`
