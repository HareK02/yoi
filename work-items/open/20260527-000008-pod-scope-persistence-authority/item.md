---
id: 20260527-000008-pod-scope-persistence-authority
slug: pod-scope-persistence-authority
title: Pod: scope 永続化 authority の整理
status: open
kind: task
priority: P2
labels: [migrated]
created_at: 2026-05-27T00:00:08Z
updated_at: 2026-05-27T00:00:08Z
assignee: null
legacy_ticket: tickets/pod-scope-persistence-authority.md
---

## Migration reference

- legacy_ticket: tickets/pod-scope-persistence-authority.md
- migrated_from: TODO.md / tickets directory migration on 2026-05-27

# Pod: scope 永続化 authority の整理

## 背景

Pod の scope は複数の場所に関連情報が存在している。

- session log の `pod.scope` extension: Pod 自身の復元用 runtime scope snapshot
- Pod metadata: Pod 名から active session/segment への pointer と spawned child 情報
- spawned child 情報: child に委譲した scope
- runtime registry: live Pod の allocation / conflict detection 用 scope
- runtime mirror: `spawned_pods.json` 等の現在プロセス向け表示・制御用情報

これらは用途が異なるが、どの情報が durable authority で、どれが live mirror / derived state なのかが読み取りづらい。特に restore、compact/fork による segment 切替、child scope の委譲・reclaim、runtime registry の再構築で、scope の保存先と復元順序が曖昧だと権限の過大復元または過小復元につながる。

## 要件

- Pod scope に関する durable authority を明確に定義する。
  - Pod 自身の base scope / effective runtime scope / deny による delegated-out 部分を区別する。
  - spawned child に委譲した scope と、親 Pod 自身の effective scope を区別する。
  - live registry / runtime mirror は durable authority ではないことを明確にする。
- Pod 名からの restore に必要な情報の保存先を一貫させる。
  - Pod 名から active session/segment を解決できる。
  - 解決した Pod が、前回終了時点の effective scope を過大に復元しない。
  - child が生存・復元対象の場合、親の delegated-out scope が意図せず reclaim されない。
- segment 遷移で scope が失われない。
  - compact / fork / resume / attach の後も、次にその segment を restore したとき同じ effective scope が得られる。
  - 新 segment 作成時に scope authority が必要なら、初期状態として確実に引き継がれる。
- spawned child の scope 永続化を親 Pod の restore/reclaim 要件と整合させる。
  - 親は child に委譲済みの scope を把握できる。
  - child 停止・shutdown・restore 時の prune により、親の effective write scope が正しく reclaim される。
  - explicit deny と delegated-out deny を混同しない。
- runtime registry 再構築時の入力と副作用を定義する。
  - restore 時にどの durable state から allocation を再作成するかが明確である。
  - stale / unreachable child を pruning した場合、durable state と runtime mirror が矛盾しない。
- 保存形式は inspect/debug しやすい。
  - Pod ごとに「active pointer」「自身の scope」「spawned child と delegated scope」が追跡できる。
  - restore 失敗時に、欠けている authority が何か分かる error になる。
- session log の conversation/history authority と scope authority の関係を明確にする。
  - scope 更新が conversation history の意味内容を汚染しない。
  - append-only session log に置く場合は、compact/fork と replay semantics 上の扱いが明示される。
  - Pod metadata に置く場合は、session/segment lineage との整合と更新順序が明示される。

## 完了条件

- Pod scope に関する durable authority / runtime mirror / derived state の責務がコードとドキュメント上で一致している。
- Pod restore が、前回の effective scope を過大復元しない regression test を持つ。
- compact または fork 後の新 segment restore で scope が失われない regression test を持つ。
- spawned child に scope 委譲済みの親 Pod を restore しても、child 側の write scope が親に二重に戻らない regression test を持つ。
- child 停止・shutdown・restore pruning 後に、親の effective scope と durable state が一致する regression test を持つ。
- runtime registry / runtime mirror が durable authority と矛盾した場合の扱いが test で確認されている。

## 範囲外

- manifest scope 設定そのものの設計変更。
- tool permission policy の allow / ask / deny 挙動変更。
- UI 表示だけで scope 不整合を隠す対応。
- 既存の壊れた手元 session log を自動修復する migration。
