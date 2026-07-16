---
title: 'StagingからMemory化する審査・剪定フローを実装する'
state: 'ready'
created_at: '2026-07-16T04:34:07Z'
updated_at: '2026-07-16T04:38:21Z'
assignee: null
---

## 背景

現状の staging は extract output の一時保存としては機能しているが、Memory 化前の審査キューとしての情報が弱い。consolidation が成功すると consumed staging は削除されるため、どの staging entry を Memory 化したか、何を捨てたか、なぜ捨てたか、何を Knowledge / Skill candidate に送るべきだったかが machine-readable に残りにくい。

この Ticket では設計だけで止めず、staging entry の resolution / disposition を実装し、consolidation が staging から Memory 化する際の審査・剪定結果を残せるようにする。Memory は短期・変化前提の context であり、staging から無制限に append しない。

関連:

- Objective `00001KVJSMQXZ`
- `.yoi/objectives/00001KVJSMQXZ/memory-architecture-overview.md`
- `.yoi/objectives/00001KVJSMQXZ/resources/pirolli-card-2005-sensemaking.md`

## 実装要件

- staging entry ごとの resolution / disposition 記録を実装する。
  - 初期実装は append-only JSONL などの軽量な形式でよい。
  - consumed staging を削除する場合も、削除前に resolution を残す。
  - invalid staging / failed consolidation は既存挙動を壊さず、可能なら diagnostic resolution を残す。
- disposition action を machine-readable にする。
  - 最低限: `discard` / `merge_memory` / `replace_memory` / `mark_memory_stale` / `delete_memory` / `defer` / `promote_to_knowledge_candidate` / `promote_to_skill_candidate` / `link_to_authority` / `create_ticket_or_doc_candidate`。
  - 初期実装で全 action を実際に mutate できない場合も、resolution として記録できるようにする。
- consolidation prompt / input / output flow を更新し、staging entry ごとの disposition を要求する。
  - discard reason / defer reason / target record / consolidation run id / consumed_by を記録する。
  - record mutation がない場合でも、なぜ Memory 化しなかったかを resolution に残せるようにする。
- Memory 化条件を prompt と実装上の記録に反映する。
  - future usefulness reason。
  - scope / applicability。
  - source anchors。
  - staleness condition / invalidation condition。
  - なぜ Knowledge / Skill / Ticket / docs ではなく Memory なのか。
  - authoritative records の単なる mirror ではないこと。
- staging からの destination routing を resolution に記録する。
  - Memory。
  - Knowledge candidate。
  - Skill candidate。
  - Ticket/doc candidate。
  - authority link only。
  - discard / defer。
- consolidation / tidy を garden operation として強める。
  - append より merge / replace / mark stale / delete を優先する guidance を prompt に入れる。
  - stale / duplicate / no-scope / no-product-use / authority-duplicate / contradictory records を検出する tidy hints を追加できる範囲で実装する。
- Pirolli & Card の sensemaking flow を implementation note / prompt に反映する。
  - shoebox: Overview + Evidence index + selected candidate ranges。
  - evidence file: source anchors 付き staging entries。
  - schemas/hypotheses: Memory 化すべきか、Knowledge/Skill に送るべきか、stale かの判断。
  - product: Memory update / Knowledge candidate / Skill candidate / Ticket/doc update / discard resolution。

## 非目標

- staging を長期 Knowledge store にしない。
- staging entry を全て Memory 化しない。
- Memory を Ticket / docs / git / session log の mirror にしない。
- 初期 slice で完全な vector DB や大規模 search infrastructure を実装しない。

## 受け入れ条件

- consolidation 実行後、staging entry ごとの disposition / resolution が durable に残る。
- consumed staging を削除する前に resolution が記録される。
- `discard` / `merge_memory` / `replace_memory` / `mark_memory_stale` / `delete_memory` / `defer` / `promote_to_knowledge_candidate` / `promote_to_skill_candidate` / `link_to_authority` / `create_ticket_or_doc_candidate` の少なくとも記録表現がある。
- Memory 化には usefulness / scope / source / staleness / destination reason が求められる。
- Knowledge / Skill / Ticket / docs への routing と、Memory に入れない重要 material の扱いが resolution として表現できる。
- resolution log / archive は staging cleanup と競合せず、consolidation lock / consumed snapshot の既存安全性を壊さない。
- resolution write、consumed cleanup、discard/defer path、no-record-change path の tests が追加または更新されている。
- `cargo test -p memory` または該当 crate の同等テストが通る。
- code/prompt/resource 変更として `nix build .#yoi` が通る。
