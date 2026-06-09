---
title: "TUI Pod list/view abstraction"
state: "closed"
created_at: "2026-05-28T14:16:02Z"
updated_at: "2026-05-28T15:40:30Z"
---

## Background

TUI で扱う Pod 関連 UI は、少なくとも次の二つの後続 ticket から使われる。

- `20260527-000017-tui-spawned-pod-panel`: spawned child Pod の一覧と一時 attach。
- `20260527-000023-multi-pod-view-ui`: 複数 Pod view を行き来する UI。

両者は表示対象や操作範囲が異なる一方で、Pod の一覧取得、status 表示、visible / attachable 判定、row 表示、選択状態、view 切り替えの土台を共有する。これを各 ticket が個別に実装すると、TUI 内で Pod list / picker / view 管理が重複し、visibility model や attach 診断がずれやすい。

まず TUI 内で用いる複数 Pod の list/view model を抽象化し、後続 UI が同じ情報構造と操作プリミティブを使える状態にする。

## Design direction

Trait 階層ではなく、source ごとの data struct を name-keyed に合成した UI model を採用する。

- `StoredPod` / `LivingPod` trait は作らない。
- `LivePodInfo` と `StoredPodInfo` は plain data struct として扱う。
- UI は `Vec<PodListEntry>` / `PodList` を読む。
- `PodListEntry` は Pod name を primary key として、`live: Option<LivePodInfo>` と `stored: Option<StoredPodInfo>` を合成した normalized row にする。
- live / stored は排他的ではない。
  - 起動中かつ stored metadata がある Pod。
  - 起動中だが durable metadata / segment がまだ薄い pending Pod。
  - stopped で stored metadata だけある Pod。
  - stored metadata が壊れている Pod。
  - registry にはあるが socket unreachable な Pod。
  これらを enum の継承的分類へ押し込めず、entry の合成状態として扱う。

この ticket で抽象化するのは list/read/merge/selection/action eligibility の土台まで。`Method::Run` の送信、attach、restore の実行そのものは入れない。

## Requirement

- TUI crate 内に Pod list 用 module を用意する。
  - 推奨名: `crates/tui/src/pod_list.rs`
  - 既存 picker の private `Row` / `PodRowState` / `LivePodRecord` / `build_rows` / metadata + registry + session summary 読み取りを、この module の model / builder へ寄せる。
- TUI が Pod 一覧 UI を構成するための共通 model / state / helper を用意する。
  - `PodList`
  - `PodListEntry`
  - `LivePodInfo`
  - `StoredPodInfo`
  - `PodVisibilitySource`
  - `PodEntryActions` または同等の action eligibility model
  - selection state（index だけでなく Pod name を primary identity として維持できること）
- `PodListEntry` は表示情報と action eligibility を持つ。
  - Pod name
  - source / visibility kind（例: resume picker, current parent spawned child, future multi-view target）
  - live reachability / `PodStatus`
  - socket path / attach target
  - stored active session / segment id
  - updated time / preview
  - stopped / unreachable / missing state / corrupt metadata の診断情報
  - `can_open`
  - `can_restore`
  - `can_send_now`
  - `can_queue_send`
  - disabled reason / diagnostic
- direct send 自体はこの ticket の範囲外だが、multi-pod view が send target 判定に使える情報は model に含める。
  - live + reachable + `PodStatus::Idle` なら `can_send_now`。
  - running は send disabled または future queue eligible として区別できる。
  - stopped は restore/open 可能だが direct send は不可。
- `tui -r` picker は新しい `PodList` / `PodListEntry` を最初の consumer として使う。
  - picker の見た目・key binding・attach/restore outcome は変えない。
  - existing picker-specific rendering は残してよいが、row data source は共有 model に寄せる。
- list row rendering / selection / refresh の責務境界を整理する。
  - TUI widget は表示と選択に寄せる。
  - Pod discovery / client protocol / registry state / session summary の取得詳細を UI 表示ロジックへ直接散らさない。
- child Pod panel と multi-Pod view UI が同じ抽象を使える設計にする。
- visibility model は変えない。
  - host-wide Pod browser を新設しない。
  - `tui -r` は既存 resume picker 相当の source だけを扱う。
  - spawned child panel は current parent から見える child Pod のみを対象にする後続 consumer として想定する。
  - multi-Pod view UI も、具体要件が決まるまではこの抽象に新しい可視範囲を勝手に足さない。
- 既存の `ListPods` / `ReadPodOutput` / `SendToPod` / `StopPod` tool semantics は変えない。
- 既存の TUI resume picker / attach flow を壊さない。

## Suggested model sketch

Exact names may differ, but implementation should keep this shape simple and data-oriented.

```rust
pub struct PodList {
    pub entries: Vec<PodListEntry>,
    pub selected_name: Option<String>,
}

pub struct PodListEntry {
    pub name: String,
    pub source: PodVisibilitySource,
    pub live: Option<LivePodInfo>,
    pub stored: Option<StoredPodInfo>,
    pub summary: PodEntrySummary,
    pub actions: PodEntryActions,
    pub diagnostics: Vec<PodEntryDiagnostic>,
}

pub struct LivePodInfo {
    pub socket_path: PathBuf,
    pub status: Option<PodStatus>,
    pub reachable: bool,
    pub segment_id: Option<SegmentId>,
}

pub struct StoredPodInfo {
    pub metadata_state: StoredMetadataState,
    pub active_session_id: Option<SessionId>,
    pub active_segment_id: Option<SegmentId>,
    pub updated_at: Option<u64>,
    pub preview: Option<String>,
}

pub struct PodEntryActions {
    pub can_open: bool,
    pub can_restore: bool,
    pub can_send_now: bool,
    pub can_queue_send: bool,
    pub disabled_reason: Option<String>,
}
```

## Acceptance criteria

- TUI crate 内に、複数 Pod list/view UI で再利用できる typed abstraction がある。
- 既存 `tui -r` picker が、その abstraction を使って rows を構成する。
- spawned child Pod list と multi-Pod view UI の後続実装が、その abstraction を使う前提で説明できる。
- Pod row の status / reachability / attach target / diagnostic 表示に必要な情報が一箇所の model にまとまっている。
- visibility scope は caller が明示的に渡すか、source kind として表現され、UI helper が host-wide enumeration を暗黙に行わない。
- selection は refresh 後も Pod name を primary identity として維持できる。
- unit test で以下が確認されている。
  - stored only row は restore/open 可能で direct send 不可。
  - live idle reachable row は open/attach 可能かつ direct send 可能。
  - live running reachable row は attach 可能だが direct send 可能とは扱わない。
  - corrupt stored metadata は diagnostic を持つ。
  - rows refresh / rebuild 後に selected Pod name が維持される。
- 既存 picker / attach 関連テストが通る。
- `cargo fmt --check`
- `cargo check -p tui -p client -p pod`
- 必要に応じて `cargo test -p tui -p pod -p protocol`

## Relationship

This is a prerequisite for:

- `20260527-000017-tui-spawned-pod-panel`
- `20260527-000023-multi-pod-view-ui`

## Out of scope

- spawned child Pod panel の完成。
- 複数 Pod view UI の完成。
- child Pod への interactive input。
- multi-Pod view からの direct send 実行。
- host-wide Pod browser。
- Pod discovery / permission / registry visibility model の変更。
- native GUI。
