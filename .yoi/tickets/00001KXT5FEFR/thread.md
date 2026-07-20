<!-- event: create author: "yoi ticket" at: 2026-07-18T08:28:54Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-18T08:29:31Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-18T08:29:31Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-18T08:29:31Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-18T08:29:31Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-18T08:32:27Z -->

## Implementation report

workdir materialization の dirty source check を `selector == "HEAD"` の場合だけ実行するようにした。

変更点:
- `LocalGitWorktreeMaterializer::materialize` で selector を先に確定。
- selector が `HEAD` の時だけ `git status --porcelain` による dirty source rejection を行う。
- 明示 branch selector では source worktree が dirty でも `git rev-parse <selector>^{commit}` で commit 解決し、detached worktree を作る。
- branch selector + dirty source の回帰テストを追加し、未コミットファイルが materialized worktree に入らないことを確認。

検証:
- `cargo test -p worker-runtime --features ws-server,fs-store branch_selector_allows_dirty_source_materialization --quiet`
- `cargo test -p worker-runtime --features ws-server,fs-store dirty_source_is_rejected_by_materialization --quiet`
- `cargo test -p worker-runtime --features ws-server,fs-store local_git_repo_materializes_detached_worktree_under_runtime_root --quiet`
- `cargo fmt --check`
- `git diff --check`


---

<!-- event: state_changed author: hare at: 2026-07-18T08:32:27Z from: inprogress to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-18T08:32:27Z status: closed -->

## 完了

Workdir dirty source rejection is now scoped to the `HEAD` selector. Explicit branch selectors can materialize detached workdirs from their resolved commit even when the source worktree has uncommitted changes.


---
