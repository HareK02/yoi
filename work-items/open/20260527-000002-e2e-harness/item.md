---
id: 20260527-000002-e2e-harness
slug: e2e-harness
title: E2E テストハーネス
status: open
kind: task
priority: P2
labels: [migrated]
created_at: 2026-05-27T00:00:02Z
updated_at: 2026-05-27T00:00:02Z
assignee: null
legacy_ticket: tickets/e2e-harness.md
---

## Migration reference

- legacy_ticket: tickets/e2e-harness.md
- migrated_from: TODO.md / tickets directory migration on 2026-05-27

# E2E テストハーネス

## 背景

`CLAUDE.md:6` で明記している通り、現状「実プロセスをスポーンさせての E2E」は未設計である。crate 内 integration test は 121 ファイル / 969 ケースまで揃っているが、以下の領域は in-process では再現できず、複数チケットの完了条件が宙に浮いている。

- `pod-cli-manifest-flags`: `--manifest` / `INSOMNIA_USER_MANIFEST` / 併用 conflict など実 CLI 挙動
- `pod-persistent-state`: Pod プロセス**再起動後**の active session 復元、spawner 再起動後の `ListPods` 復元
- `pod-session-fork`: `pod_cli` から fork → 新 session で run まで通せる
- `pod-parent-turn-callback`: 実子 Pod を spawn した状態の親 history 反映
- `pod-empty-turn-rollback`: 「TUI / pod_cli いずれの経路でも」明記
- `permission-extension-point.review.md:21`: `[permissions]` を含む Pod 構築 → tool deny までの結合検証
- `llm-worker-stream-continuation`: SSE 途中切断 + 継続/中断と課金重複が無いことの確認
- `native-gui-mvp`: GUI から `pod` subprocess を起動 → socket 接続 → graceful shutdown

`crates/pod/tests/spawn_pod_test.rs` のように subprocess を `/bin/true` ですり替える擬似手法は既にあるが、これは「子 Pod が即終了する状況下での registry 書き込み」を見るためのもので、実 pod を立ち上げての protocol 往復はしていない。

## 方針

- ワークスペース直下 **`tests/e2e/`** に E2E 専用の crate を切る。E2E は単一の crate / バイナリの責務ではないため、既存 `crates/<x>/tests/` には置かない。
- 実 `pod` バイナリは `env!("CARGO_BIN_EXE_pod")` で取得。ファイルシステムは tmpdir に閉じ、`INSOMNIA_RUNTIME_DIR` / data dir / `INSOMNIA_USER_MANIFEST` 等を env で完全 sandbox 化する。
- protocol を喋る側は **`tickets/client-crate.md` で切り出す `client` crate** を直接利用する。TUI バイナリを PTY で叩く方針は採らない（GUI MVP との整合と E2E 安定性の観点から）。
- CI 既定実行から外す。`--features e2e` か独立ジョブで opt-in。ローカルでは `cargo test -p e2e --features e2e` 相当で叩ける形にする。

## 詰めたい論点（実装前に決める）

### 1. LLM provider のスタブ手段が fixture HTTP 再生だけで充足するか

既存 `crates/llm-worker/tests/anthropic_fixtures.rs` 等は in-process loader として書かれており、HTTP サーバーとして再生する形にはなっていない。E2E では Pod プロセスが env で渡された URL に対して実 HTTP を叩く以上、**最低限「fixture を返す HTTP サーバー」** は必要。

ただし、それだけで充足するかは不明:

- **動的応答が要るシナリオ**: SSE を途中で能動的に切る (`llm-worker-stream-continuation`)、tool 呼び出しの結果に応じて分岐する応答、複数ターンに渡る会話の途中で挙動を変える、など。録画再生だけでは作りにくい。
- **provider 差**: Anthropic / OpenAI Responses / Gemini / Ollama / Codex OAuth で endpoint / 認証 / スキーマが違う。E2E で全 provider を回す必要は無いが、最低 1〜2 provider はハーネスを持たせるべきで、選定が要る。
- **OAuth 系**: Codex OAuth はトークン取得経路自体が外部依存。E2E では事前注入された token を読む形に倒すか、OAuth flow ごと canned server で模すか。

このチケットでは「fixture HTTP 再生」を出発点としつつ、**動的応答のための最小 canned server インターフェース**（テストケース側からハンドラを差し替えられる形）も同時に検討範囲に含める。両方が無いと上のシナリオが書けない。

### 2. provider URL の差し替え経路

各 provider の base URL を env で上書きできる前提が、現コードに揃っているか確認・整備する必要がある。揃っていなければ別チケットに切り出すか、本チケット内で minimal に対応するか決める。

### 3. fixture 形式

既存の in-process fixture (`tests/*_fixtures.rs`) と HTTP 再生用 fixture を同じソースから作るか、別管理にするか。共通化できるなら record/replay 経路を整備する。

### 4. 並列実行と env 干渉

`spawn_pod_test.rs` は env mutex で直列化している。E2E でも env (`INSOMNIA_*`)・runtime dir・socket path に依存する以上、テスト並列度の方針を決める（`--test-threads=1`、test-per-process、または env を引数にハンドオフして mutex 不要にする）。

### 5. 失敗時の診断

実プロセスが絡むためスタックトレースだけでは原因特定しにくい。pod の stderr / stdout、session log、runtime dir の中身をテスト失敗時に dump する仕組みを最初に入れておく。

## 要件

- `tests/e2e/` 以下に E2E 用 crate（仮称 `e2e`）が存在し、`Cargo.toml` の `[features] e2e = []` で gate されている。
- `cargo test -p e2e --features e2e` で実 `pod` バイナリを spawn し、protocol 経由で 1 シナリオ（最小: spawn → 1 turn 実行 → graceful shutdown）が通る。
- LLM provider のスタブが少なくとも 1 provider 分動き、上の最小シナリオが本物の HTTP 越しに完結する。
- env / tmpdir / socket path が tmpdir 内に閉じ、テスト間の干渉が無い。
- テスト失敗時に pod プロセスの stderr / 関連ファイルが artefact として確認できる。
- CI 既定パス (`cargo test --workspace`) では E2E が走らない。opt-in jobs でだけ走る。
- 上の論点 1〜5 が文書化されている（チケット内 or `docs/` 配下のいずれか）。

## 完了条件

- 上記要件を満たすハーネスが入り、最小シナリオ 1 本が通る。
- 後続シナリオ（permission deny / cli-manifest-flags / spawn 親子 / resume / fork / stream-continuation）を**書く側の手順書**が提示されている（fixture 追加方法、シナリオ crate の追加方法、env のお作法）。
- 個別シナリオの実装は本チケットに含めない。後続チケットで切る。

## 範囲外

- 個別 E2E シナリオの実装（permission deny / cli flags / spawn / resume / fork / stream-continuation）。それぞれ後続チケット。
- 全 provider 分の HTTP スタブ（最初は 1 provider に絞る）。
- TUI バイナリを PTY で操作する経路。
- GUI バイナリの E2E（`tickets/native-gui-mvp.md` 完了後に別途）。
- E2E を CI 既定で走らせる切替。

## 依存 / 関連

- `tickets/client-crate.md`（protocol を喋る client crate を切り出す。E2E はここに依存して書く）
- `tickets/llm-worker-stream-continuation.md`（動的応答 canned server を必要とする最初のシナリオ）
- `tickets/permission-extension-point.review.md`（最初に書きたいシナリオ）
- `crates/pod/tests/spawn_pod_test.rs`（env mutex 等の流儀を流用）
- `crates/llm-worker/tests/*_fixtures.rs`（fixture 資産の出発点）
- `CLAUDE.md:6`（E2E 未設計の宣言）
