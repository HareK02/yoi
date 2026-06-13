---
title: 'TUI rewind picker の Enter 後に live 表示が巻き戻らない問題を調査・修正する'
state: 'inprogress'
created_at: '2026-06-13T09:23:07Z'
updated_at: '2026-06-13T11:14:26Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['tui', 'pod-protocol', 'persistence', 'history-rewind']
queued_by: 'workspace-panel'
queued_at: '2026-06-13T10:53:20Z'
---

## Background

通常 TUI の manual rewind は、`Ctrl+R` で rewind targets 画面を開き、対象 user message を選択して `Enter` で Pod-authoritative に巻き戻し、選択 input を composer に復元する仕様である。

実運用中に、rewind targets 画面で `Enter` を押しても画面上は無反応に見え、巻き戻らないことがあると観測された。追加情報として、他のキーは効き、`Enter` を押した後に `Esc` で戻ると、押した回数分だけ `Rewound session:` が表示される。また、一度 `Ctrl+X` で TUI を落としてから Restore すると、巻き戻し後状態として効いている。Pod の状態は通常の停止状態のはずだった。

既存関連 Ticket:

- `00001KSKBPBX0` — Pod/TUI: 手動 rewind 導線（closed）
- `00001KSKBPGS8` — Pod: 任意ターンからの Fork（複数ターン巻き戻し）（planning、今回の不具合とは別件）

## Observed behavior

- `Ctrl+R` で rewind targets 画面を開く。
- 上下移動など、他のキーは効く。
- 対象上で `Enter` を押しても画面上は無反応に見える。
- その後 `Esc` で rewind targets 画面を閉じると、押した `Enter` の回数分だけ `Rewound session:` が表示される。
- `Ctrl+X` で TUI を落としてから Restore すると、巻き戻し後の状態として効いている。
- Pod の状態は通常の停止状態、少なくとも Running 中ではないはずだった。

## Investigation notes

read-only 調査で以下を確認した。

- `Ctrl+R` / picker 中の `Enter` は `crates/tui/src/single_pod.rs` の key handling から `app.submit_rewind_picker()` に到達し、`Method::RewindTo` を返す。
- `submit_rewind_picker()` は `Method::RewindTo { target, expected_head_entries }` を返すだけで、picker を閉じず、applying/pending 状態も持たず、追加 `Enter` を抑止しない。
- Pod 側 `Method::RewindTo` は `crates/pod/src/controller.rs` で Idle 時に `apply_rewind()` され、成功すると `Event::RewindApplied` と `Event::Status { Idle }` を送る。
- `Rewound session:` という文字列は Pod 側ではなく `crates/tui/src/app.rs` の `Event::RewindApplied` handler でのみ生成される。したがって、`Esc` 後にこの表示が出る時点で、Pod 側 rewind は成功し、TUI も最終的には `Event::RewindApplied` を処理している。
- `Event::RewindApplied` handler は、`self.greeting.clone()` が `Some` の場合だけ `restore_snapshot(&entries, greeting)` する。`self.greeting == None` の場合、rewind 後 `entries` payload は transcript restore に使われないが、success alert は push される。
- `restore_snapshot()` は `self.blocks.clear()` する。複数回 `RewindApplied` を処理しても、restore が動いていれば古い `Rewound session:` alert は消えるはずである。`Enter` 回数分 alert が残る観測は、`restore_snapshot()` が呼ばれていない可能性、特に `self.greeting == None` の可能性と整合する。

現時点の有力仮説は、Pod 側 rewind は成功しているが、live TUI が `Event::RewindApplied` の `entries` を使って derived transcript view を再構築できておらず、reconnect/Restore 時の fresh `Event::Snapshot` で初めて表示が正しくなる、というもの。

別途、`Event::RewindApplied` の処理または描画反映が rewind picker 表示中に進まず、`Esc` などの terminal event 後の drain でまとめて処理されている可能性も調査する必要がある。

## Requirements

- `Ctrl+R` で開いた rewind targets view において、eligible な target 上で `Enter` を押した場合、Pod 側 rewind 成功後に live TUI の transcript/composer/view state が直ちに一貫した巻き戻し後状態へ更新される。
- Pod 側 rewind は成功しているのに live TUI が古い transcript 表示のまま残り、TUI restart/Restore で初めて正しい状態になる挙動を解消する。
- `Event::RewindApplied` の `entries` restore が `App::greeting` 欠落等で silently skip されないようにする。
- `RewindApplied` が picker 表示中に処理される場合でも、picker が適切に閉じるか、少なくともユーザーが次に行うべき操作が分かる状態に遷移する。
- apply が拒否または restore 不可能な場合は、無反応ではなく可視 diagnostic / notice を出す。
- `Enter` 連打により同じ target への `RewindTo` が複数積まれ、後から `Rewound session:` がまとめて出る挙動を防ぐ。
- 既存の manual rewind 仕様を維持する:
  - picker 開始は Idle / Paused の既存仕様を尊重する。
  - apply は Pod-authoritative に検証・適用する。
  - Running 中は拒否する。
  - picker 表示時から head が変わった場合は apply 時に再検証して拒否する。
  - 成功時は composer が空なら選択 message を composer に復元する。
  - 選択だけでは auto-run しない。

## Acceptance criteria

- 再現条件または失敗条件が実装報告に説明されている。
- `Ctrl+R` → target 選択 → `Enter` で Pod 側 rewind が成功した場合、TUI restart/Restore なしに live TUI の transcript が巻き戻し後状態へ更新される。
- `Event::RewindApplied` に含まれる `entries` が、`App::greeting` 欠落等の理由で silently ignored されない。
- `self.greeting == None` または同等の metadata 欠落が起き得る場合、その経路を修正するか、self-contained event / fresh snapshot request / explicit diagnostic など設計上妥当な挙動にする。
- rewind picker 表示中に `Enter` 成功 event が来た場合、`Esc` 後に初めて `Rewound session:` が出るのではなく、その場で view state / composer / status が一貫して更新される。
- `Enter` 連打が複数 rewind request や成功 notice の後出し表示を生まない。必要なら pending/applying 状態で追加 submit を抑止する。
- apply 不可または restore 不可の場合は、無反応ではなく actionbar / diagnostic / error event 等で理由が見える。
- 既存の Esc cancel、Running 中 rejection、stale-head rejection、composer restore の挙動を壊さない。
- 関連する TUI key handling / rewind view / Pod protocol path の focused test が追加または更新されている。

## Binding decisions / invariants

- TUI がローカルに履歴を削るのではなく、rewind 適用は引き続き Pod が authoritative に検証・適用する。
- Pod 側 rewind が成功したのに live TUI が stale view のまま残る UX は許容しない。
- `Event::RewindApplied` の restore failure を silently skip しない。
- この Ticket では fork / alternate history は実装しない。
- Tool side effect の undo は実装しない。
- rewind semantics は `00001KSKBPBX0` の既存仕様を前提にし、必要な場合のみ不具合修正として局所的に調整する。

## Implementation latitude

実装者は原因調査の結果に応じて、以下のいずれかまたは複数を修正してよい。

- `Event::RewindApplied` を self-contained にするため、必要な metadata（例: greeting/status 等）を event に含める。
- `App::greeting` が `None` になり得る経路を修正する。
- `RewindApplied` restore 不可時に fresh snapshot を要求する、または明示的 diagnostic を出す。
- rewind picker に applying/pending state を追加し、submit 後の二重 `Enter` を抑止する。
- TUI event loop / socket delivery / wake-up に、picker 表示中の Pod event 処理遅延がある場合は修正する。
- focused tests を追加するために、既存 helper の分離や再利用を行う。

ただし、manual rewind の authority boundary と destructive rewind semantics は変更しない。

## Readiness

- readiness: implementation_ready
- risk_flags: [tui, pod-protocol, persistence, history-rewind]

## Escalation conditions

- `Event::RewindApplied` を self-contained にするために protocol schema / compatibility への明示判断が必要になった場合。
- `App::greeting` 欠落が broader snapshot / restore / connection lifecycle の設計問題だった場合。
- dedicated view 表示中の Pod events / notices の扱いが TUI 全体の UX/architecture 判断を必要とする場合。
- current active segment / compacted segment / stale head の扱いについて、既存 Ticket の仕様と矛盾する判断が必要になった場合。
- Pod-authoritative rewind ではなく TUI 側ローカル mutation に寄せる設計変更が必要に見える場合。

## Validation

- focused test: `Event::RewindApplied` により live TUI transcript が rewind 後 `entries` で reseed され、picker が閉じる。
- focused test: `App::greeting` 欠落または metadata 欠落時に silently skip せず、設計した failure/recovery path が動く。
- focused test: rewind picker submit 後の二重 `Enter` が複数 `Method::RewindTo` を生成しない、または idempotent/rejected として可視化される。
- focused test: success 後に composer が空なら selected input が復元され、非空なら既存 composer を上書きしない。
- 必要に応じて event loop / pod event wake-up の focused test を追加する。
- `cargo fmt --check`
- `cargo check -p protocol -p pod -p tui`
- 関連 focused tests

## Related work

- `00001KSKBPBX0` — Pod/TUI: 手動 rewind 導線
- `00001KSKBPGS8` — Pod: 任意ターンからの Fork（複数ターン巻き戻し）
