---
title: 'Workspace Browserから手動Coding Workerを作成する導線を追加する'
state: 'inprogress'
created_at: '2026-07-02T12:59:57Z'
updated_at: '2026-07-02T17:01:11Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-07-02T16:13:24Z'
---

## 背景

Workspace Browser には Runtime / Worker catalog と Worker Console はあるが、ユーザーが Browser から「普通に読み書きできる coding agent Worker」を明示作成する導線がまだ無い。

現状の `workspace-server` には Runtime worker spawn の backend 経路があり、Browser-facing API としても `/api/runtimes/{runtime_id}/workers` が存在する。ただし、この既存 endpoint は Runtime create に近い field を受け取り得るため、Browser UI から `ConfigBundleRef`、requested capabilities、raw workspace path、cwd、secret、runtime store path などを直接渡す導線にしてはいけない。

この Ticket では、Workspace sidebar の WORKER セクションから明示的に New form を開き、Backend が coding preset を解決して Worker を作成し、作成後に Worker Console へ遷移する導線を作る。

## 目的

- Workspace Browser から手動で Coding Worker を作成できるようにする。
- Sidebar の WORKER 見出し横に `New` button を置く。
- Browser-facing request は product-level launch request とし、Runtime internal create request を直接露出しない。
- Backend が coding preset から Profile / ConfigBundle / execution backend / workspace scope を解決する。
- 作成成功後は Worker Console に遷移する。
- 作成失敗時は sanitized diagnostic を form に表示する。

## UI 設計

### Sidebar entry point

`web/workspace/src/lib/workspace-sidebar/WorkersNavSection.svelte` の WORKER section heading 横に `New` button を追加する。

```text
WORKER        New
```

`New` を押すと Worker 作成 form を開く。v0 は sidebar 内の inline panel でも modal/dialog でもよいが、既存 sidebar UX を壊さないこと。

### Form fields

v0 の form は次の 4 field に限定する。

- `display_name`
  - optional。
  - Worker identity ではなく表示名だけに使う。
- `runtime_id`
  - 対象 Runtime。
  - default: embedded Runtime。
  - 候補は `/api/workspace` など既存 projection から `can_spawn_worker=true` の Runtime を使うか、v0 は embedded Runtime 固定でもよい。
- `profile`
  - Worker の振る舞いを選ぶ Profile。
  - default: coding 用 Profile。
  - Browser の自由入力ではなく、Backend が公開する候補から選ぶ。
- `initial_text`
  - optional。
  - 作成直後の user input として Worker に渡す。

`kind = coding` はこの endpoint の launch mode として Backend が持ってよいが、form の入力 field にはしない。

自由入力にしないもの:

- raw workspace path。
- cwd。
- tool scope。
- `ConfigBundleRef`。
- `requested_capabilities`。
- secret / token。
- Runtime endpoint / socket path / session path / store path。

### Success behavior

Worker 作成成功後:

1. Worker list を refresh する。
2. 作成された Worker の Console に遷移する。

```text
/runtimes/{runtime_id}/workers/{worker_id}/console
```

### Error behavior

Worker 作成失敗時:

- form 内に sanitized diagnostic を表示する。
- raw path、secret、Runtime endpoint、internal store path を表示しない。
- form の入力値は保持する。

## Backend API 設計

Browser から既存 Runtime create payload を直接叩かせず、Workspace Backend に product-level endpoint を追加する。

候補:

```http
POST /api/workers
```

request v0:

```json
{
  "runtime_id": "embedded-worker-runtime",
  "display_name": "scratch worker",
  "profile": "builtin:coder",
  "initial_text": "このリポジトリを確認して"
}
```

response:

```json
{
  "runtime_id": "embedded-worker-runtime",
  "worker_id": "...",
  "worker": { ... existing WorkerSummary or WorkerDetail projection ... }
}
```

### Backend resolution

Backend は manual Coding Worker launch endpoint として、request の `profile` を次へ解決する。

- Profile 候補に存在するか検証する。
- Profile から default ConfigBundle を解決 / sync / check する。
- embedded / selected Runtime spawn を行う。
- normal workspace read/write capable Worker execution path に接続する。
- initial text を Worker input / initial input として送る。

Browser-facing request に `kind`、ConfigBundle identity、requested capabilities を持たせない。

### 既存 endpoint との関係

- `/api/runtimes/{runtime_id}/workers` は internal-ish Runtime spawn endpoint として残ってよい。
- Browser の New Worker UI は新しい `/api/workers` product-level endpoint を使う。
- 将来的に `/api/runtimes/{runtime_id}/workers` を debug/admin/internal に寄せるかは別 Ticket で扱う。

## Coding Worker の意味

この Ticket の Coding Worker は、一般的な coding agent と同様に workspace 内のファイルを読み書きできる Worker を指す。

ただし Browser が file path / scope / tool grant を直接指定するのではなく、Backend が現在の Workspace Backend 実行環境に基づいて既存 execution backend / tool host の範囲へ接続する。

v0 では embedded Runtime / local execution backend を対象にする。remote Runtime で workspace provisioning が未対応の場合は typed diagnostic で拒否してよい。

## 実装要件

- `WorkersNavSection.svelte` の heading 横に `New` button を追加する。
- New form を追加し、display name / runtime / profile / initial text を入力できる。
- Form submit で Browser-facing `/api/workers` launch endpoint を呼ぶ。
- Backend に `/api/workers` POST endpoint を追加する。
- Backend request type は product-level launch request とし、Runtime create request をそのまま deserialize しない。
- Backend が request の `profile` を default coding Worker launch に解決する。
- Browser-facing request/response に raw workspace path、cwd、ConfigBundleRef、requested capabilities、secret、Runtime endpoint、store path を含めない。
- 作成成功後に Worker Console へ遷移する。
- 作成失敗時に sanitized diagnostic を表示する。
- Worker list refresh を行う。

## 受け入れ条件

- Sidebar WORKER heading 横に `New` button がある。
- `New` で Worker 作成 form が開く。
- Form には display name / runtime / profile / initial text だけがある。
- Form から coding Worker を作成できる。
- Form request は `/api/workers` product-level endpoint を使う。
- Browser-facing payload に raw path / cwd / `ConfigBundleRef` / requested capabilities / secret / Runtime endpoint / store path が含まれない。
- Backend が request の `profile` から Profile / ConfigBundle / execution backend を解決する。
- 作成成功後、作成された Worker の Console に遷移する。
- 作成失敗時、sanitized diagnostic が表示される。
- remote Runtime など未対応対象では typed diagnostic で拒否される。
- Focused tests が Backend launch endpoint、payload sanitization、success projection、unsupported runtime diagnostic、UI form rendering / submit path を確認する。
- `cd web/workspace && deno task test` が通る。
- `cd web/workspace && deno task check` が通る。
- `cargo test -p yoi-workspace-server` が通る。
- `cargo check -p yoi` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。

## 対象外

- Remote Runtime workspace provisioning の完成。
- per-worker cwd / tool scope の新 UI。
- ConfigBundle editor UI。
- Profile picker の本格実装。
- multi-agent Ticket role launch UI。
- Worker reconnect / stale recovery UI。
- `/api/runtimes/{runtime_id}/workers` の削除。
