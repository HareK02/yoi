---
title: 'Remove Knowledge support'
state: 'inprogress'
created_at: '2026-07-15T20:04:08Z'
updated_at: '2026-07-15T22:41:26Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-07-15T20:27:02Z'
---

## 背景

Knowledge は `.yoi/knowledge`、`KnowledgeQuery`、`#<slug>` reference、resident Knowledge injection、Workflow `requires` dependency などとして残っているが、現状の dogfooding ではほぼ使われていない。再利用可能な手順・参照資源は Agent Skills / `SKILL.md` と typed feature/tool surface へ寄せる方針にするため、Knowledge 固有機能を先に削除して概念境界を単純化する。

Memory は durable summary / decisions / requests として残すが、Knowledge は separate record kind としては廃止する。

## 要件

- `.yoi/knowledge` を active workspace record authority として扱わない。
- model-visible Knowledge tools を削除する。
  - `KnowledgeQuery` を削除する。
  - `MemoryRead` / `MemoryWrite` / `MemoryEdit` / `MemoryDelete` の `kind = knowledge` を削除する。
- LLM prompt / tool guidance から Knowledge 利用指示を削除し、Memory / Ticket / Skill / repository files の使い分けに更新する。
- resident Knowledge injection を削除する。
  - `model_invokation` / `ResidentKnowledgeEntry` / resident knowledge collection を削除する。
  - compaction / rehydration / prompt rendering に Knowledge section が残らないようにする。
- TUI / protocol の `#<slug>` Knowledge reference と completion を削除する。
  - parser / chips / styling / completion kind / system item handling を整理する。
  - `#` sigil が不要なら未使用または将来予約として扱う。
- Workflow の required Knowledge dependency は、Workflow removal Ticket 側で消える前提とし、残る intermediate code では Knowledge dependency を増やさない。
- memory lint / memory consolidation / memory extraction から Knowledge record kind を削除または無効化する。
  - consolidation prompt は Knowledge 作成・更新を指示しない。
  - memory linter は `.yoi/knowledge` を対象にしない。
- docs / reports / README / crate docs の Knowledge 前提を整理する。
  - historical report は必要ならそのままでよいが、current design docs は Knowledge を active feature として説明しない。
- 既存 `.yoi/knowledge` data がある場合の扱いを明示する。
  - 初期実装では migration code を増やしすぎず、ignore / diagnostic / manual archive のいずれかにする。
- Skills support Ticket は Knowledge removal 後に進める。
  - Skill は Knowledge replacement ではなく、procedural guidance の first-class resource として扱う。

## 受け入れ条件

- `KnowledgeQuery` と `kind = knowledge` が model-visible tool schema から消えている。
- Worker prompt / resident context / compaction / rehydration に Knowledge section が出ない。
- TUI / protocol から `#<slug>` Knowledge ref path が削除されている、または明示的に unsupported になる。
- memory lint / consolidation が Knowledge record を作成・更新・検査しない。
- current docs で Knowledge が active supported feature として説明されていない。
- Skills support Ticket `00001KXKMX0QM` がこの Ticket を prerequisite として扱う。
- affected crates の `cargo test` と `nix build .#yoi` が通る。

