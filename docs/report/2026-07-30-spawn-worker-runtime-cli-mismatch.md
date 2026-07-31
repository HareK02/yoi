# SpawnWorker が current `yoi-runtime` CLI と不整合で起動できない

Date: 2026-07-30

## 発生状況

Workdir selector/ref変更の未コミット差分をreviewer Workerへ委譲するため、`SpawnWorker`をread-only scopeで呼び出した。

## 結果

Worker socketが作成されず、child stderrには以下が記録された。

```text
yoi-runtime: unexpected positional argument `worker`

Usage: yoi-runtime [OPTIONS]
```

現在の`yoi-runtime`はRuntime REST serverの直接起動CLIであり、`worker` positional subcommandを受け付けない。SpawnWorker側のlauncherが旧CLI契約を使っている可能性がある。

## 影響

- reviewer/coder Workerをspawnできず、今回の差分は親Worker内で実装・検証した。
- scope delegationやreviewer profile以前にprocess起動で失敗するため、Worker orchestration機能が利用できない。

## 改善案

SpawnWorker launcherがcurrent Worker/Runtime起動contractを使用しているかを確認し、CLI rename後のdirect executable契約と同期する。失敗時には実際に組み立てたargvと解決したexecutable pathもbounded diagnosticとして返すと、installed binary/worktree binaryの取り違えを判別しやすい。
