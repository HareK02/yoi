---
title: 'Ticket と Objective の ID を base32 timestamp 形式に統一する'
state: 'queued'
created_at: '2026-06-09T07:30:47Z'
updated_at: '2026-06-09T10:35:08Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-09T10:31:17Z'
---

## 背景

Ticket identity simplification と Objective record 導入により、Ticket / Objective の canonical ID を共通の短い不透明 ID に揃えたい。

現状:

- Ticket は過去の timestamp + slug 形式から移行中。
- Objective は `YYYYMMDD-HHMMSS-001` 形式で、秒単位 timestamp + counter を使っている。

望ましい方向:

- ID は title / slug / 内容を含まない。
- lexicographic sort が作成時系列として使える。
- 完全連番は Git branch / workspace 並行作業で衝突しやすいため避ける。
- 秒単位 + `-001` ではなく、millisecond timestamp を短い固定幅 text に圧縮する。
- `created_at` は人間可読な timestamp field として別に持つ。

## ゴール

Ticket と Objective の canonical ID を、共通の fixed-width base32 encoded Unix epoch milliseconds 形式に統一する。

## 要件

- Ticket と Objective の canonical ID 形式を共通化する。
- ID は Unix epoch milliseconds を base32 text にしたものにする。
- alphabet は人間が読み間違えにくいものを使う。
  - 例: Crockford base32 系 `0123456789ABCDEFGHJKMNPQRSTVWXYZ`。
  - `I` / `L` / `O` など紛らわしい文字は避ける。
- ID は fixed width にする。
  - 例: 9 chars。
  - fixed width により lexicographic sort が chronological sort と一致する。
- ID は title / slug / content words を含まない。
- ID は canonical path component として安全であること。
- `created_at` は引き続き frontmatter field として持つ。
  - ID は compact primary key。
  - `created_at` は人間可読な作成時刻。
- `updated_at` も existing behavior として維持する。

## Collision handling

- ID 生成時に同じ workspace 内で path collision が起きた場合、timestamp milliseconds を `+1ms` して再 encode する。
- 衝突時に `-001` / suffix / random tail を付けない。
- `+1ms` を繰り返して空き ID を探す。
- 生成に使った adjusted timestamp と `created_at` の関係を明確にする。
  - 推奨: `created_at` は実作成時刻を保持し、ID は collision-adjusted primary key として扱う。
  - もしくは adjusted timestamp を `created_at` にも反映する場合は、その理由を明記する。
- 過剰衝突時の bounded error を用意する。

## Scope

- 共通 ID allocator/helper を作る。
  - Ticket create path と Objective create path で共有する。
  - 将来の project records にも使える形が望ましい。
- Ticket create / path layout をこの ID 形式に移行する。
- Objective create / path layout をこの ID 形式に移行する。
- Existing Ticket / Objective records を migration する。
- `TicketList` / `TicketShow` / `Objective list` / `Objective show` / doctor / tests を更新する。
- Relation metadata や Objective linked tickets は、この canonical ID を前提にする。

## 非目標

- ID から作成時刻を人間が直接読めるようにすること。
  - 必要なら `created_at` を見る。
- 完全連番 ID。
- title / slug を ID に含めること。
- random UUID を使うこと。
- この Ticket で Ticket relation metadata や Objective の高機能化を実装すること。

## 受け入れ条件

- Ticket と Objective の新規作成で同じ canonical ID format が使われる。
- ID は fixed-width base32 epoch-milliseconds text で、title/slug を含まない。
- ID の lexicographic sort が作成時系列と一致する。
- 同一 millisecond collision は `+1ms` retry で解決される。
- `created_at` と `updated_at` が frontmatter に残る。
- Existing Ticket / Objective records が新 ID/path に migration される。
- Objective の `YYYYMMDD-HHMMSS-001` 形式は current format ではなくなる。
- Ticket の old timestamp-slug path / slug-derived identity は current format ではなくなる。
- Tests cover:
  - encoding/decoding or ordering property;
  - collision handling;
  - Ticket create;
  - Objective create;
  - doctor validation;
  - migration-relevant lookup behavior.
- `target/debug/yoi ticket doctor`, `target/debug/yoi objective doctor`, focused tests, `cargo fmt --check`, and `git diff --check` pass.
