# AI maintainer 用 WorkItem / Thread 抽象メモ

## 位置づけ

AI maintainer が単なる coding agent ではなく、作業の発見・分解・実装委譲・review・完了判断まで扱うには、現在の `TODO.md` / `tickets/*.md` だけでは足りない。必要なのは、個々の file path ではなく「作業単位」と「その会話・判断・成果物」を扱う抽象である。

このメモは実装 ticket ではなく、将来こういうものが必要になるという設計メモとして置く。具体的な最初の実装は `tickets.sh` / `work-items/` の MVP で試す。

## 必要になりそうな概念

- WorkItem
  - 作業単位。title / status / kind / priority / labels / acceptance criteria / links を持つ。
- Thread
  - WorkItem に紐づく append-only な会話・判断・review・実装報告の流れ。
- Event
  - comment / plan / decision / implementation_report / review / status_change など。
- Artifact
  - review log、test log、設計メモ、branch / commit / worktree への link など。
- Lease / Run
  - どの Pod / agent がどの worktree / scope で作業中かを表す runtime coordination 情報。

## 配置の分担

repo-managed な project-visible 領域には、作業の正本として人間が読める coordination data を置く。

```text
repo/
  work-items/   # WorkItem / Thread / Artifact
  tickets/      # 当面の既存 ticket。将来 WorkItem view に寄せる候補
  docs/         # plan / report
```

`.insomnia` は local runtime state として扱い、project coordination の正本にはしない。

```text
.insomnia/
  memory/
  workflow/
  maintainer/
    leases/
    runs/
    inbox/
```

## ID と backend の考え方

WorkItem ID は中央 `SEQUENCE` にしない。複数 branch / worktree / Pod が同時に作業を作ると conflict しやすいため、timestamp + slug などの衝突しにくい ID にする。

```text
YYYYMMDD-HHMMSS-<slug>
YYYYMMDD-HHMMSS-<short-rand>-<slug>
```

backend は最初は repo 内 file backend でよいが、抽象としては Git directory 固定にしない。将来、remote maintainer hub や GitHub Issues などへ移る可能性を潰さない。

## 移行イメージ

1. 既存の `TODO.md` / `tickets/` 運用は維持する。
2. `tickets.sh` MVP で `work-items/` の file backend と thread 操作を試す。
3. 既存 `TODO.md` / `tickets/` を手動で `work-items/` に寄せ、`doctor` が通る状態にする。
4. その運用が安定したら、`TODO.md` を generated view にするか、廃止するかを判断する。
5. 必要になった段階で Rust crate / Store interface / LeaseStore / remote backend を検討する。

## 当面やらないこと

- remote maintainer hub の実装。
- SQLite / index / search daemon の導入。
- Pod lifecycle / completion tracking の完全実装。
- LeaseStore の本格実装。
- TUI 統合。

## 参照

- `tickets/tickets-sh-workitem-thread-mvp.md`
- `docs/plan/ai-maintainer.md`
- `tickets/auto-maintain-workflow.md`
