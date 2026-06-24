---
title: 'Pod/session storage cleanup CLI を追加する'
state: 'done'
created_at: '2026-06-24T11:39:41Z'
updated_at: '2026-06-24T12:36:12Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['pod-lifecycle', 'persistence', 'destructive-operation', 'cli-ux', 'session-history', 'authority-boundary']
queued_by: 'workspace-panel'
queued_at: '2026-06-24T12:01:42Z'
---

## User claims / request snapshot

- Pod 名を指定して Pod を消去する CLI コマンドを実装する。
- Pod から参照されていないセッションを削除するコマンドを用意する。
- 古い Pod / セッションを削除するコマンドを用意する。
- コマンド名は以下で合意済み:
  - `yoi pod delete <NAME> [--force] [--dry-run]`
  - `yoi pod prune [--older-than <DURATION>] [--force] [--dry-run]`
  - `yoi session prune --unreferenced [--older-than <DURATION>] [--force] [--dry-run]`

## Confirmed facts / sources

- 関連 closed Ticket `00001KTJ7MACG` は fresh-start 専用 UX を実装しない方針で closed 済み。ただし resolution で、Pod 名を明示的に空ける archive/delete と storage cleanup/prune commands は必要になれば別 Ticket として扱う、と記録されている。
- `crates/pod-store/src/lib.rs` では Pod metadata authority は `{data_dir}/pods/{pod_name}/metadata.json` であり、`PodMetadataStore::delete_by_name` / `FsPodStore::delete_by_name` が既に存在する。
- `crates/session-store/src/fs_store.rs` / `crates/session-store/src/lib.rs` では session log は `{sessions_root}/{session_id}/{segment_id}.jsonl` と trace jsonl で、`FsStore` は `list_sessions`, `list_segments`, `lookup_session_of` を持つ。確認範囲では削除用 public API は見当たらない。
- `crates/yoi/src/main.rs` では `yoi pod ...` は現在 `pod::entrypoint::run_cli_from("yoi pod", args)` に委譲されており、Pod runtime spawn/restore 用 entrypoint として使われている。
- `crates/yoi/src/session_cli.rs` では `yoi session ...` は現在 `analyze <SESSION_JSONL_PATH> --json` のみ。
- `crates/pod/src/entrypoint.rs` では `--pod <NAME>` は name-keyed Pod state があれば restore、なければ fresh create する。sessions dir は `--store` または `manifest::paths::sessions_dir()`、Pod store は `manifest::paths::data_dir()/pods` から解決している。
- duplicate / related search では、直接同じ目的の open Ticket は見つからなかった。関連 closed Ticket は `00001KTJ7MACG`, `00001KSTRGFX0`, `00001KSXXRRC8`。

## Unverified hypotheses

- live Pod protection は runtime socket / Pod registry / discovery layer と組み合わせて判定できる可能性が高いが、実装時に正確な helper/API を確認する必要がある。
- orphan session 判定は「active Pod metadata から参照されない SessionId」を基本にできそうだが、fork/segment lineage、trace file、古い closed metadata、manual session restore use case をどこまで保存対象にするかは実装時に注意する必要がある。
- `yoi pod delete` を runtime entrypoint に足すより、product CLI 側で `yoi pod delete` を捕捉するほうが安全かもしれないが、既存 `yoi pod` の意味と衝突しない設計確認が必要。

## Undecided points / open questions

- Blocking open question は現時点ではない。
- 実装中に CLI boundary が既存 `yoi pod` runtime entrypoint と衝突する場合は、Orchestrator / maintainer に escalate する。
- orphan session 判定で session lineage semantics や manual restore use case への影響が大きい場合は、削除範囲を安全側に狭めるか follow-up に分ける。

## Background

以前の fresh-start Ticket では、restore bypass 専用 path を作らず、Pod 名の衝突解消や古い履歴蓄積は Pod/session storage cleanup commands として扱う方針になった。現在も Pod metadata と session logs は durable authority が分かれており、手で `~/.yoi/pods` や `~/.yoi/sessions` を消すのは危険で discoverable ではない。公式 CLI と safety rails が必要。

## Requirements

- Pod 名を指定して、name-keyed Pod metadata を削除できる CLI を追加する。
  - コマンド名は `yoi pod delete <NAME> [--force] [--dry-run]` とする。
  - 削除後、同じ Pod 名の通常起動は既存の `missing -> spawn fresh` path に乗る。
  - session logs/history は `pod delete` では削除しない。
- live/reachable Pod の metadata 削除は拒否する。
  - 実装が live 判定できない場合は安全側に倒し、削除せず診断する。
  - live Pod を止める操作や stop semantics の変更はこの Ticket の主目的にしない。
- orphan session cleanup CLI を追加する。
  - コマンド名は `yoi session prune --unreferenced [--older-than <DURATION>] [--force] [--dry-run]` とする。
  - Pod metadata の active pointer から参照されていない session/segment を候補として列挙できる。
  - 実削除は `--force` が必要。
  - 初期実装は dry-run / report を重視し、削除対象と根拠を表示する。
- old Pod / old session cleanup CLI を追加する。
  - Pod 側は `yoi pod prune [--older-than <DURATION>] [--force] [--dry-run]` とする。
  - 「古い」の判定は暗黙 default を持たせず、`--older-than <DURATION>` のような明示 threshold を要求する。
  - threshold 未指定で「古い」と判断して削除しない。
- destructive operation は共通で:
  - default は dry-run または削除拒否にする。
  - 実削除には `--force` を要求する。
  - 削除対象、保持対象、拒否理由を bounded に表示する。
- product CLI help/docs を更新し、manual data-dir surgery ではなく公式 command を案内する。
- `yoi pod` が既存 runtime entrypoint として使われている境界を壊さない。
  - management subcommands を product CLI 側で捕捉するか、runtime entrypoint 側に安全に追加するかは実装判断でよいが、既存 spawn/restore options と互換的に動くこと。

## Acceptance criteria

- `yoi pod delete <NAME>` 相当の公式 CLI で stopped/restorable Pod metadata を削除できる。
- 削除後、同名 Pod 起動は existing metadata restore ではなく fresh create path になる。
- live/reachable Pod に対する delete/prune は拒否され、理由が表示される。
- `pod delete` は session logs を削除しない。
- Pod metadata から参照されていない session を dry-run で列挙でき、`--force` 指定時だけ削除できる。
- 古い Pod / session cleanup は `--older-than` 等の明示条件なしでは削除しない。
- prune/delete の出力は、削除予定/削除済み/保持/拒否をユーザーが確認できる。
- focused tests が以下を cover する:
  - stopped/restorable Pod delete success。
  - live Pod delete refusal。
  - Pod delete 後の same-name missing/fresh behavior。
  - referenced session is preserved。
  - unreferenced session prune dry-run and force behavior。
  - old Pod/session prune requires explicit threshold。
  - CLI parsing/help。

## Binding decisions / invariants

- No silent restore bypass. Fresh same-name start は、ユーザーが明示的に metadata を削除した結果としてのみ発生する。
- Pod metadata delete の副作用として session history を自動削除しない。
- live/reachable Pod metadata を削除しない。
- 暗黙の “old” threshold を持たせない。old Pod/session cleanup では age criteria をユーザーが明示する。
- destructive deletion には明示的な `--force` を要求し、dry-run/report behavior を提供する。
- Pod metadata authority は `pod-store`、session log authority は `session-store` のままにする。
- legacy top-level resume flags や bare Pod-name inference を再導入しない。
- この Ticket では Panel/TUI を broad Pod manager にしない。CLI 実装で十分とし、Panel integration は必要なら follow-up に分ける。

## Implementation latitude

- 合意済み command spelling は維持する。ただし clap/parser 上の曖昧さを避けるための minor option shape 調整は、既存 UX を壊さない範囲で許容する。
- product CLI 側で management subcommands を捕捉するか、runtime entrypoint 側へ安全に追加するかは実装判断でよい。
- 必要なら product CLI から使う shared cleanup module を作ってよい。
- 必要なら `session-store` に削除 API を追加してよい。その場合は path safety と trace/log cleanup の tests を追加する。
- Orphan session detection は初期実装では active `PodMetadata.active.session_id` references を authority としてよい。より高度な lineage-aware retention は、referenced sessions を削除しない限り follow-up に分けてよい。
- Output は初期実装では human-readable でよい。JSON output は周辺 CLI と自然に揃えられる場合を除き follow-up でよい。

## Readiness

- readiness: implementation_ready
- risk_flags: [pod-lifecycle, persistence, destructive-operation, cli-ux, session-history, authority-boundary]

## Escalation conditions

以下の場合は実装を進める前に Orchestrator / maintainer に escalate する。

- live Pod detection を安全に拒否できるほど reliable にできない。
- orphan detection が session lineage semantics の変更を必要とする。
- Pod delete の副作用として sessions を削除する必要が出る。
- storage migration や compatibility fallback が必要になる。
- command design が `yoi pod` runtime entrypoint usage と衝突する。
- cleanup が Panel role-session/Ticket claims、worktrees、branches、Ticket state を mutate しようとする。

## Validation

- `cargo fmt --check`
- focused `cargo test` for `yoi`, `pod-store`, `session-store`, and affected Pod/discovery code
- `cargo check -p yoi -p pod -p pod-store -p session-store`
- `target/debug/yoi ticket doctor` または `yoi ticket doctor`
- `git diff --check`

Reviewer は path safety、live/refusal behavior、dry-run/force semantics、session history を accidental に削除しないことを重点確認する。

## Related work

- Related closed Ticket: `00001KTJ7MACG` — Add Pod archive and fresh-start path.
- Related closed Ticket: `00001KSTRGFX0` — Split Pod metadata into a dedicated pod-store crate.
- Related closed Ticket: `00001KSXXRRC8` — Pod tools: unify pod listing and rename restore operation.
- Files inspected:
  - `crates/yoi/src/main.rs`
  - `crates/yoi/src/session_cli.rs`
  - `crates/pod/src/entrypoint.rs`
  - `crates/pod-store/src/lib.rs`
  - `crates/session-store/src/fs_store.rs`
  - `crates/session-store/src/lib.rs`
