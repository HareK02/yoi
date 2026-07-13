---
title: 'Type Workdir cleanup plan status'
state: 'done'
created_at: '2026-07-13T12:45:21Z'
updated_at: '2026-07-13T12:56:55Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-13T12:46:05Z'
---

## 背景

Runtime と workspace-server はどちらも Rust であり、Runtime catalog の Workdir status / cleanliness は `worker-runtime` の typed enum として共有できる。それにもかかわらず cleanup plan は `file_status: String` / `cleanliness: String` に落として判定しており、`WorkingDirectoryStatusKind::NotFound` が `notfound` になるバグを起こした。

Rust 内部の cleanup policy 判定では文字列化せず、typed enum / 明示的 parse 境界に閉じる。

## 要件

- `CleanupWorkdirCandidate.file_status` を文字列ではなく typed enum にする。
- `CleanupWorkdirCandidate.cleanliness` を文字列ではなく typed enum にする。
- Runtime observation 由来の Workdir status / cleanliness は `worker-runtime` の型から typed に変換する。
- DB registry 由来の status / cleanliness は明示的な parse 境界で typed に変換し、未知値は `unknown` として扱う。
- cleanup action 判定は string match ではなく enum match にする。
- JSON API では従来通り snake_case string として serialize される。

## 受け入れ条件

- `not_found` Workdir が型付き判定で `workdir_record_delete` になる。
- `clean` Workdir が型付き cleanliness 判定で `workdir_clean_cleanup` になる。
- Rust 内部 cleanup policy に `format!("{:?}").to_lowercase()` や status string match が残らない。
- workspace-server tests と Nix build が通る。
