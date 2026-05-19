# Review: session-store: Session（Segment 群の grouping）導入

対象コミット: `a6cc05f feat: Session（Segment 群の grouping）を導入`
ベース: `develop` 上の `e4cda5d Merge: segment-rename`

## 前提・要件の確認

- **新 `SessionId` 型を `session-store` に追加。Segment は `parent_session_id` を持つ**:
  達成。`SessionId` 型 alias と `new_session_id()` が
  `crates/session-store/src/lib.rs:65-73`。
  `LogEntry::SegmentStart { session_id, .. }` が
  `crates/session-store/src/segment_log.rs:43`（serde 上は必須フィールド、`#[serde(default, skip_serializing_if = ..)]` の付かない裸の `session_id`）。`RestoredState.session_id: Option<SessionId>` は
  `segment_log.rs:202` に追加され、`collect_state` で `SegmentStart` から拾われる（`segment_log.rs:258`）。

- **Segment 生成パスの session_id 決定（new / compaction / fork）**:
  - new session: `create_segment` (`segment.rs:30`)、`Pod::new` (`pod.rs:495`)、`Pod::from_manifest` (`pod.rs:2786`)、`from_manifest_spawned` (`pod.rs:2867`) が `new_session_id()` を発行。`session_store::fork` (`segment.rs:406`) も同じく新発行。
  - compaction: `create_compacted_segment` (`segment.rs:75`) と `Pod::compact` (`pod.rs:2188`) が `source_session_id` を継承。
  - in-Session fork: `fork_at` (`segment.rs:454`) と `ensure_segment_head` の auto-fork (`pod.rs:1689`) が `source_session_id` を継承。
  - `Pod::from_manifest_spawned` は spawn 元と独立した新 Session を発行 → child Pod は親 Pod と別 conversation 扱い。設計上の明確化は欲しいが、本 ticket の要件範囲では妥当。

- **`SessionOrigin` の整理（`compacted_from` / `forked_from`、同 Session 内 segment への参照）**:
  実装は `SegmentOrigin { segment_id, at_turn_index }` を保持し（`segment_log.rs:181`）、両 origin は `LogEntry::SegmentStart` の `forked_from` / `compacted_from` フィールドに入る（`segment_log.rs:50, 54`）。同 Session 内参照は **コード経路で保証**（`create_compacted_segment` / `fork_at` の構築時 `source_session_id` 引数を経由する形）であって、**型レベルでは保証されていない**。ticket 要件「型レベルで保証」とは厳密には乖離する（非ブロッキング、後述）。

- **restore API を `(SessionId, SegmentId)` に**:
  達成。`Store::*` 全 method が `(session_id, segment_id)` を取る (`store.rs:46-99`)。`restore` (`segment.rs:97`) も同様。`SegmentId` のみを取る legacy shim は `restore_by_segment` (`segment.rs:111`) として一段噛ませてあり、内部で `Store::lookup_session_of` を呼ぶ。pod-cli `--session` (`pod/src/main.rs:188`) と TUI の `load_resume_scope` (`tui/src/spawn.rs:332`) がこの shim を経由している。

- **FsStore layout に Session 単位の index**:
  達成。`<root>/<session_id>/<segment_id>.jsonl` で Session ディレクトリ配下に Segment file を並べる (`fs_store.rs:5-8`)。`list_segments(session_id)` (`fs_store.rs:123`) は当該 Session ディレクトリ scan で済むので `WHERE session_id = ?` 相当。

- **Session 内の leaf Segment 列挙 API**:
  `Store::list_segments(session_id)` (`store.rs:63`) が UUID v7 の lexicographic 順（時系列順）で最新→古い順に返す (`fs_store.rs:142`)。leaf は先頭 1 件で取れる（picker で実際にそうしている: `picker.rs:96-101`）。専用の `latest_segment_of` 等は無いが、`list_segments(sid).first()` で十分な最小実装。

- **migration 戦略**:
  ticket 要件は「既存 sessions ディレクトリは破棄して良いかどうかをここで明示」。実装は新 layout (`<root>/<session_id>/<segment_id>.jsonl`) に切り替え、旧 flat `<root>/<segment_id>.jsonl` は `list_sessions` (`fs_store.rs:103-121`) の dir-type filter で自動的に skip されて見えなくなる。即ち「実質破棄」されている。**コード上の comment ではこの方針が明示されておらず**（`fs_store.rs:1-8` の Layout doc には触れていない）、ticket 完了条件「明示」が docs/code どちらにも乗っていない点で部分達成。

- **`cargo check --workspace` + 全テスト pass**:
  確認。`cargo check --workspace` は `Finished dev` まで通る。`cargo test --workspace -- --skip double_run_returns_error` も全 suite green（warning 6 件、後述 nits）。`double_run_returns_error` は KNOWN_ISSUES.md 既登録の flake。

## アーキテクチャ・スコープ

- **層境界の維持**: 新 `SessionId` 型は session-store の最下層に置かれ、pod / pod-cli / TUI / pod-registry / session-metrics が同じ型を利用する形。`llm-worker` には Session 概念が滲んでいない（cache-key として segment_id を渡すのみ）。低レベル基盤としての session-store の役割を保っている。
- **`SegmentLocation` の atomic swap 設計**: `(session_id, segment_id)` を `ArcSwap<SegmentLocation>` で一括 swap (`pod.rs:48-94`)。fork / compaction で session_id と segment_id を**同時に**更新する必要があるため、別々の atomic 変数に分けると中間状態を観測される。1 struct で wrap した設計は妥当。`set_segment_id` の置き換え (`set_location`) もシグネチャ上 (session_id, segment_id) ペアを取るので安全。
- **`SegmentOrigin` の同 Session 性**: 上述の通り型レベルでは保証されない（`SegmentOrigin` 単体は `(segment_id, at_turn_index)` のみ）。妥当性を以下で評価:
  - もし型レベルで保証するなら `SegmentOrigin { session_id, segment_id, at_turn_index }` にし、`SegmentStart.session_id == forked_from.session_id` の比較を `Deserialize` 後に走らせる方向、あるいは phantom-typed の compile-time invariant が必要。前者は redundant な persistence、後者は Rust の表現力で書くのが重く割に合わない。
  - 現実問題、`SegmentOrigin` の構築点は `create_compacted_segment` / `fork_at` / `ensure_segment_head` の 3 経路に限定され、いずれも source の session_id を継承するルートのみ通る。新規 callsite が追加されない限り invariant は破れない。CLAUDE.md の「設計を歪めない」原則と整合する範囲では、現状の構築時保証 + doc comment 明示で実用上十分。
  - ticket 要件文言「型レベルで保証」は厳密にはアンメットだが、実装方針として **「同 Session 性は構築時保証、doc で invariant 明示」** に倒した判断は妥当。ticket 文言を改めるか、現状を accept するかは user 判断。
- **`Pod::from_manifest_spawned` の session_id 新規発行**: spawn された child Pod が親と独立した Session を持つのは、conversation grouping の意味論として妥当（child Pod は別 conversation を成す）。ただし spawn 元の親子関係を Session 永続表現が持たないと、後で「child の Session を親の Segment と紐付ける」操作（spawn linkage）が必要になる可能性がある。範囲外。
- **`lookup_session_of` の O(N) ディレクトリ走査**: shim 用途のみで、pod-cli 起動時に 1 回、TUI resume 時に 1 回呼ばれる程度。Session 数が極端に多い（10^5 オーダー）と起動時 latency に効くが、ユーザーが実際に保持する Session 数（典型 10^2 オーダー）では問題にならない。picker は使わない（picker は session_id を直接持っている）。実装はディレクトリ存在 check で短絡 (`fs_store.rs:147`) しているので最悪ケースも軽い。妥当。
- **TUI picker の Session 単位列挙への変更**: 以前は Segment 単位列挙 → 各 row 1 segment。現在は Session 単位 + 各 Session の leaf segment（最新）のみ preview。compaction で複数 Segment ができている Session でも 1 row。これは「同じ会話の家系」をユーザーが認識する単位として正しい UX。ticket の「Session 内の leaf Segment 列挙 API を提供」と整合（picker 経由で実際に leaf を選んで resume している）。
- **過剰実装の確認**: live-fork marker、Pod 単位 metadata、TUI Session/Segment 選択 UI、DB backend は範囲外。いずれも実装に踏み込んでおらず、最小実装に留まっている。
- **依存追加なし**: 既存型 (`uuid::Uuid`、`ArcSwap`) のみで構築。`cargo add` ポリシー違反なし。

## 指摘事項

### Blocking

- なし。

### Non-blocking / Follow-up

- **`SegmentOrigin` の同 Session 性が型レベルでは保証されない**:
  `crates/session-store/src/segment_log.rs:181` の `SegmentOrigin` は `segment_id` と `at_turn_index` のみで、`session_id` を持たない。ticket 要件「型レベルで保証」とは厳密に乖離する。
  - 実用上は構築点が `create_compacted_segment` / `fork_at` / `ensure_segment_head` に限定され、いずれも source の session_id を継承する経路のみ通るため invariant は維持されているが、`Deserialize` 経由で外部から不整合 payload を流し込めば破れる。
  - 厳密化するなら `SegmentOrigin { session_id, segment_id, at_turn_index }` に拡張し、`collect_state` 時に `SegmentStart.session_id` との一致を assert する手も。ただし persistence が冗長になるトレードオフがある。
  - 後続 ticket（fork tree の永続表現が増える `live-fork-marker` 等）で扱うか、現状の「構築時保証 + doc 明示」で十分とするかは設計判断。

- **migration 方針がコード上で明示されていない**:
  ticket 完了条件「既存 sessions ディレクトリは破棄して良いかどうかをここで明示」が code/doc で明示されていない。
  - `crates/session-store/src/fs_store.rs:1-8` の Layout doc に「旧 flat layout (`<root>/<segment_id>.jsonl`) は本実装では認識されず、`list_sessions` は dir entries のみ列挙する。後方互換は意図的に作らない」旨を 1 行足す程度で十分。
  - ticket 上は説明されているので、`tickets/session-grouping-introduce.md` を grep して runtime 文脈に書き出す形でも可。

- **`PickerOutcome::Picked.session_id` フィールドが未使用**:
  `crates/tui/src/picker.rs:67` で `session_id: SessionId` を返すが、`crates/tui/src/main.rs:213` は `Picked { segment_id, .. }` で `session_id` を捨てている。
  - `cargo check` の `field is never read` warning として表面化。
  - 現状 TUI resume は `--session <segment_id>` のみを子 pod に渡し、pod-cli 側で `lookup_session_of` で解決し直すフローなので picker が session_id を返してもパススルーされる先がない。
  - 解決策: (a) `Picked { segment_id }` のみにする / (b) TUI から子 pod に session_id も渡せるようフラグを足す（pod-cli 側で `lookup_session_of` を省略できる）。short-term は (a)、long-term は (b)。
  - 同様に `picker.rs:339` の `short_segment(id: SessionId)` は名前が `short_segment` のまま（表示対象は session_id）。previous review の nit と同類の取り残し。

- **`tools::Tracker` / `TaskStore` の "session-scoped" / "session-lifetime" コメント残り**:
  segment-rename 後続コミット (`7577577`) で header コメントは `Pod-lifetime` に統一されたが、内部コメントに `session-scoped` / `session-lifetime` が残存:
  - `crates/tools/src/tracker.rs:21` "A `Tracker` is **session-scoped**: ..."
  - `crates/tools/src/lib.rs:10,45` "session-lifetime, enforces..." / "(session-lifetime)"
  - `crates/tools/src/task.rs:3,254,260,263,266` "Pod/session-lifetime state" / "session-lifetime task"
  - `crates/pod/src/pod.rs:803,811,823` "session-scoped file-operation tracker" / "session-scoped TaskStore"
  - `crates/pod/src/compact/worker.rs:5` "session-lifetime `Tracker`"
  - 本 ticket で **本物の Session 概念**が入った今、これらの comment は誤読を招く（読み手が「これは新 SessionId に紐づくのか？」と疑う）。実体は Pod プロセス寿命なので `Pod-lifetime` / `Pod-scoped` に統一すべき。
  - segment-rename の review follow-up として一部対応されたが取り残しが多いので、本 ticket かこの後ろの「pod-lifetime 用語整理」ticket でまとめて清掃するのが良い。

- **`picker.rs:194-212` の `last_message_preview` が `AssistantItems`（旧 plural）のみ match**:
  本 ticket の前 (`segment-rename` 時点) からの取り残しだが、新規 write が `AssistantItem` (singular) のみ使う運用に変わったため、picker preview は assistant 発言を拾えなくなり実質「user 発言 or `[empty]`」のみになる。本 ticket で picker UX を Session 単位に変えた今、preview 品質の劣化が表面化しやすい。`LogEntry::AssistantItem`、`LogEntry::SystemItem`、`LogEntry::ToolResult` への対応追加が望ましい。

### Nits

- **`session_test.rs` のテスト名**: `session_run_logs_entries`、`session_restore_round_trip`、`session_resume_after_pause`、`session_run_with_tool_call`、`session_fork_creates_new_session`、`session_fork_at_truncates_within_session`、`session_config_changed_logged`、`session_auto_forks_on_conflict`（`tests/session_test.rs`）。中身は新 `SessionId` を引数で受けて `restore`/`fork` する正当な Session 概念のテストになっているので、名前と中身は整合的。ファイル名 `session_test.rs` も今となっては妥当。previous review の nit からは解消方向。

- **`Pod::from_manifest_spawned` で child Pod が新 Session を起こす意図**: コードコメントが明示していない。`pod.rs:2867` の周辺に「spawn された child は独立した conversation grouping を持つ（親と Session を共有しない）」旨の 1 行を残しておくと、後で「spawn linkage を session 上で表現するか」検討する時の根拠が消えにくい。

- **`Pod::session_id()` の安定性 doc**: `pod.rs:572-576` で「Stable across compaction and in-Session fork — only `fork` changes it」と書かれているが、コード上の `fork()` (brand-new Session の `session_store::fork` を内部で呼ぶ Pod-level API) は実装されていない。`session_store::fork` を直接呼ぶフリー関数のみ存在し、`Pod` の method としては自身の `session_id` を入れ替える操作が今のところ無い（auto-fork も compaction も同 Session 維持）。doc は将来の Pod-level fork を見据えた spec だが、現実装範囲では「`session_id()` は Pod 寿命を通じて不変」と書いた方が現状認識として正確。

- **`SegmentState` のメソッド `set_location` vs 旧 `set_segment_id`**: previous review で `set_session_id` → `set_segment_id` のリネームが follow-up として履行されたが、本 ticket で更に `set_location(SegmentLocation)` に変わった（segment_id + session_id を atomic swap する正しい形）。旧 `set_segment_id` は callsite 全消し済み。整合的。

## 判断

**Approve with follow-up** — 必須要件（`SessionId` 型導入 / Segment 生成パスの session_id 決定 / `(SessionId, SegmentId)` restore / FsStore layout / leaf 列挙 / `cargo check` + 全テスト pass）は満たされ、ticket の最小実装範囲を守っている。

懸念点は (1) `SegmentOrigin` の同 Session 性が「型レベル保証」ではなく「構築時保証 + doc 明示」に留まる点（ticket 文言と乖離するが、実装方針として CLAUDE.md の「設計を歪めない」原則とは整合）、(2) migration policy がコード上で文字に起こされていない点、(3) `PickerOutcome.session_id` の dead field 残り、(4) picker preview の旧 plural-only 対応、(5) tools / pod の "session-scoped" コメント残り。いずれも blocking ではなく、後続 ticket か小さな follow-up commit で清掃可能。
