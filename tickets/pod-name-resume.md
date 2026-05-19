# Pod state: Pod 名単位の resume / attach 導線

## 背景

`pod-state-write-points` で Pod state が active session を保持するようになる。本チケットでは pod-cli / TUI 側から **Pod 名で resume / attach できる入口**を確定し実装する。

既存の `--session <UUID>` resume は引き続き使えること。

## 要件

- pod-cli の引数仕様を確定:
  - 例: `pod --pod <name>` で manifest cascade から同名 Pod state を引いて resume
  - `--session` との同時指定時の優先順位 / エラーを明示
- 解決順序: Pod 名 → Pod state → active `(SessionId, SegmentId)` → session restore。
- Pod state が存在しない pod 名で起動した場合: 新規 Pod として作成 (initial Pod 起動と同じパス)。
- TUI 側の入口は本チケットでは「最小限の resume / attach 導線」のみ。Pod 一覧 UI や history UX は別チケット。
- 衝突検出: Pod 名が既に live で running なら pod-registry が検知して reject する既存挙動を維持。

## 完了条件

- pod-cli の `--pod` (名称は実装時確定) で resume できる。
- TUI から Pod 名で attach する最小経路が動作する。
- `--session <UUID>` resume が壊れていない。
- `cargo check --workspace` および `cargo test -p pod-cli -p pod` が通る。

## 範囲外

- TUI 上の Pod 一覧 UI / fork tree 可視化。
- spawn された子 Pod 一覧の復元（別チケット `spawned-registry-persist`）。
- Session 単位 / Segment 単位の resume 引数（本チケットでは Pod 名から内部解決のみ）。

## 関連

- `tickets/pod-state-backend.md` (前提)
- `tickets/pod-state-write-points.md` (前提)
- `crates/pod-cli/`
- `crates/pod-registry/`
