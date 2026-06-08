---
id: 20260527-000006-permission-default-policy
slug: permission-default-policy
title: 'Permission: allow-all 既定 policy への整理'
status: open
kind: task
priority: P2
labels: [migrated]
workflow_state: planning
created_at: 2026-05-27T00:00:06Z
updated_at: 2026-05-27T00:00:06Z
assignee: null
---

## Migration reference

- legacy_ticket: tickets/permission-default-policy.md
- migrated_from: TODO.md / tickets directory migration on 2026-05-27

# Permission: allow-all 既定 policy への整理

## 背景

現在の tool permission は `[permissions]` セクションが無い場合に permission 層を無効化し、`[permissions]` がある場合だけ `default_action` を必須としている。

実行時の意味として、未指定時の挙動はほぼ `default_action = "allow"` と同じであり、`Option<ToolPermissionConfig>` による「無効」と allow-all policy が型上で分かれていることが仕様理解と実装の分岐を増やしている。

## 要件

- resolved manifest の permission は常に policy として存在する形に整理する。
- 既定 policy は allow-all とし、`default_action = "allow"` かつ rule なしと同等にする。
- manifest に `[permissions]` が無い既存ユーザー設定は従来通り全ツール実行可能にする。
- `default_action = "deny"` による allowlist 型運用と、`default_action = "allow"` + deny rule による blocklist 型運用を明確に維持する。
- merge/parse 用の partial config では、「その層が permissions に触れていない」ことを表現できるようにする。

## 方針

`PodManifest` のような resolve 後の型では `permissions: ToolPermissionConfig` を持ち、`ToolPermissionConfig::default()` を allow-all とする。

`PodManifestConfig` / partial 側では階層 manifest の merge semantics のために `Option<PermissionConfigPartial>` を残してよい。resolve 時に未指定を allow-all default policy へ畳み込む。

`[permissions]` セクションを書いた場合の `default_action` 必須制約は見直す。rule だけを書いた場合は `default_action = "allow"` と解釈できるようにするか、明示必須を維持する場合でも resolved 型上は allow-all default と矛盾しない形にする。

## 完了条件

- resolve 後の manifest から permission policy の `Option` が消えている。
- `[permissions]` 未指定時に allow-all policy が得られる。
- permission rule 評価と Pod への built-in hook 登録が、常在 policy 前提で単純化されている。
- manifest resolve / merge / permission hook のテストが新しい既定値をカバーしている。
- docs の `[permissions]` 説明が allow-all 既定であることを明記している。
