---
description: yoi プロジェクトで child git worktree を作成・管理するための機械的手順。coder Pod に作らせず、orchestrator Pod が main workspace で実行する。
model_invokation: true
user_invocable: true
requires: []
---
# Worktree Workflow

yoi プロジェクトで実装差分を main workspace から分離するため、`./.worktree/<task-name>` に child git worktree を作る。これは **worktree の扱い方だけ** を定める Workflow であり、ticket 選定、coder / reviewer sibling の起動、外部レビュー、merge の運用は `$user/multi-agent-workflow` 側で扱う。

yoi では Pod の write scope が排他的に委譲されるため、child Pod の write scope は child worktree に限定する。child worktree は Yoi project records marker として tracked `.yoi` records を含んでよいが、generated/personal memory root `.yoi/memory`、local override、runtime state、logs、locks、secret-like files は出さない。main workspace は active orchestration progress と最終 review/approval/close の authority として残す。

## 適用範囲

この Workflow は親 Pod / 下位 orchestrator が main workspace で実行する。

- coder Pod にこの Workflow を渡して worktree を作らせない。
- coder Pod は、orchestrator が作成済みの child worktree を受け取り、その中で実装・build・test・報告を行う。
- reviewer Pod は、coder Pod の子ではなく orchestrator 配下の sibling として、原則 read-only で main workspace と child worktree を読む。
- ticket 作成、active orchestration progress、最終 review/approval/close は main workspace 側で扱う。
- branch-local artifacts / dossiers / docs/report / tracked `.yoi` project records は、実装対象に必要なら child worktree 内で扱ってよい。

## 原則

- 1 ticket / 1 実装 task につき 1 worktree を作る。
- 複数 ticket を下位 orchestrator に任せる場合も、実装差分は ticket / bounded task ごとに worktree を分ける。
- worktree path は `./.worktree/<task-name>`。
- branch 名は原則 `<task-name>` と同じ kebab-case。
- child worktree には `.yoi` project records を出してよい。
- child worktree では `.yoi/memory`、local/runtime/log/lock/secret-like paths を sparse checkout で除外する。
- active orchestration progress と最終 review/approval/close は main workspace 側で扱う。branch-local artifacts/dossiers は child worktree 内に置いてよい。
- push はしない。

## 事前確認

作成前に以下を確認する。

1. 対象 ticket / task が決まっているか。
2. `<task-name>` が branch / path 名に使える kebab-case か。
3. `git worktree add` を実行してよい許可があるか。
4. main workspace に混ぜてはいけない未保存差分がないか。
5. 同名 branch / worktree が既に存在しないか。
6. coder / reviewer を sibling として扱う orchestrator が誰か明確か。

同名 branch がある場合は、既存 branch を使うか、人間に確認する。`git worktree add -b` で上書きしない。

## 作成手順

main workspace で実行する。

```bash
git worktree add .worktree/<task-name> -b <task-name>

git -C .worktree/<task-name> sparse-checkout init --no-cone
git -C .worktree/<task-name> sparse-checkout set --no-cone \
  '/*' \
  '!/.yoi/memory/' \
  '!/.yoi/memory/**' \
  '!/.yoi/logs/' \
  '!/.yoi/logs/**' \
  '!/.yoi/_logs/' \
  '!/.yoi/_logs/**' \
  '!/.yoi/locks/' \
  '!/.yoi/locks/**' \
  '!/.yoi/**/*.log' \
  '!/.yoi/**/*.lock' \
  '!/.yoi/**/.lock' \
  '!/.yoi/override.local.toml' \
  '!/.yoi/**/*.local' \
  '!/.yoi/**/*.local.*' \
  '!/.yoi/local/' \
  '!/.yoi/local/**' \
  '!/.yoi/runtime/' \
  '!/.yoi/runtime/**' \
  '!/.yoi/pods/' \
  '!/.yoi/pods/**' \
  '!/.yoi/sessions/' \
  '!/.yoi/sessions/**' \
  '!/.yoi/sockets/' \
  '!/.yoi/sockets/**' \
  '!/.yoi/tmp/' \
  '!/.yoi/tmp/**' \
  '!/.yoi/cache/' \
  '!/.yoi/cache/**' \
  '!/.yoi/secrets/' \
  '!/.yoi/secrets/**' \
  '!/.yoi/**/*.secret' \
  '!/.yoi/**/*.secret.*'
```

この sparse-checkout は `.yoi` 自体を除外しない。`.yoi/memory` は generated/personal memory marker として child worktree から外す。memory root detection の実装変更はこの Workflow では扱わない。

確認する。

```bash
git -C .worktree/<task-name> status --short --branch
test ! -e .worktree/<task-name>/.yoi/memory
if test -d .worktree/<task-name>/.yoi; then
  test ! -e .worktree/<task-name>/.yoi/override.local.toml
  test -z "$(find .worktree/<task-name>/.yoi \
    \( -path '*/_logs' -o -path '*/logs' -o -path '*/locks' \
       -o -path '*/local' -o -path '*/runtime' -o -path '*/pods' \
       -o -path '*/sessions' -o -path '*/sockets' -o -path '*/tmp' \
       -o -path '*/cache' -o -path '*/secrets' -o -name '*.log' \
       -o -name '*.lock' -o -name '.lock' -o -name '*.local' \
       -o -name '*.local.*' -o -name '*.secret' -o -name '*.secret.*' \) \
    -print -quit)"
fi
```

この確認は `.yoi` project records の存在を失敗扱いしない。`.yoi/memory` と local/runtime/log/lock/secret-like paths が出ていないことを確認する。

失敗した場合は、worktree / branch / lock の状態を確認し、勝手に cleanup せず人間へ報告する。

## Pod へ渡す scope

Pod を使う場合、Pod の cwd は main workspace のままになる。必ず作業対象が child worktree であることを明示し、Bash 実行時は毎回 `cd <repo>/.worktree/<task-name> && ...` させる。

coder Pod 推奨 scope:

```text
read:  <repo>
write: <repo>/.worktree/<task-name>
```

reviewer Pod 推奨 scope:

```text
read: <repo>
read: <repo>/.worktree/<task-name>  # main workspace の read に含まれるなら別指定不要
```

reviewer は原則 write scope を持たない。review artifact を書かせる必要がある場合だけ、ticket artifacts など限定 directory を write scope として渡す。

より狭く切れる場合は、coder の write scope を変更対象 crate / directory まで狭めてよい。ただし build / test に必要な生成物を書けることを確認する。

## child worktree 内の禁止事項

- `.yoi/memory` を作らない / コピーしない / 復元しない。
- local overrides、runtime sockets/state、Pod session mirrors、cache/tmp、logs、locks、secret-like files を作らない / コピーしない / commit しない。
- main workspace の `TODO.md` / `tickets/` / `docs/report/` / `.yoi` を編集しない。
- active orchestration progress と最終 review/approval/close を child worktree 内だけで完結させない。
- 実装対象に必要な tracked `.yoi` project records、branch-local artifacts / dossiers / docs/report は child worktree 内で扱ってよい。
- merge / push / branch deletion / worktree remove をしない。
- scope / permission / history persistence / prompt context 加工原則に関わる設計変更を無断で行わない。

## 完了時の扱い

worktree 作成 Workflow としては、完了時に merge しない。merge、ticket 完了、TODO 削除は `$user/multi-agent-workflow` または人間の明示指示で行う。

coder Pod へ渡す完了報告項目の標準形:

- worktree path
- branch 名
- commit hash（coder Pod に commit を許可した場合）
- 変更ファイル
- 実装概要
- 実行した build / test / format
- 未解決事項
- review に回せるか

reviewer Pod へ渡す完了報告項目の標準形:

- 読んだ ticket / intent packet / diff
- 実際のコード変更の概念的説明
- intent / requirements / invariant との対応
- blocker / non-blocker / follow-up
- validation の妥当性
- 親または上位 orchestrator が判断すべき残論点
