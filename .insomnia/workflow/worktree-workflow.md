---
description: Git worktree を使って実装用作業ツリーを作り、main workspace の管理ファイルと子 worktree のコード差分を分離して開発を進める
model_invokation: false
user_invocable: true
requires: []
---
# Worktree Workflow

Git worktree を使って、実装差分を main workspace から分離して進める。main workspace は ticket / review / inbox / `.insomnia` を持つ制御面、子 worktree はコード変更専用の作業面として扱う。

この Workflow は `.claude/skills/worktree-workflow/SKILL.md` の運用を insomnia 向けに移植したもの。insomnia では Pod の write scope が排他的に委譲されるため、子 worktree に `.insomnia` を置かず、親 Pod が main workspace 側の管理ファイルを書ける状態を保つ。

この Workflow は親 Pod / orchestrator が worktree を用意するための手順である。実装 Pod にこの Workflow を渡して worktree を作らせてはならない。実装 Pod は親 Pod から既存 child worktree を受け取り、その中で実装・build・test・報告だけを行う。

## 原則

- 1 セッション / 1 ticket / 1 task につき 1 worktree を作る。
- worktree は `./.worktree/<task-name>` に作る。
- branch 名は原則 `<task-name>` と同じにする。
- 子 worktree はコード差分専用にし、`TODO.md` / `tickets/` / `docs/report/` / inbox などの管理ファイルは main workspace 側で扱う。
- 子 worktree には `.insomnia` を出さない。worktree 作成後に sparse checkout で `.insomnia` を除外する。
- git commit / merge / push / branch deletion / worktree remove は、人間が明示した場合以外は行わない。

## 事前確認

作業前に以下を確認する。

1. 対象 ticket / task 名が決まっているか。
2. branch / worktree 名に使える kebab-case の `<task-name>` があるか。
3. git 書き込み操作を行ってよい明示許可があるか。
4. main workspace の未保存差分や既存 worktree と衝突しないか。

許可が曖昧な場合、`git worktree add` の前に人間へ確認する。

## 作成手順

`<task-name>` を確定したら、main workspace で以下を実行する。

```bash
git worktree add .worktree/<task-name> -b <task-name>

git -C .worktree/<task-name> sparse-checkout init --no-cone
git -C .worktree/<task-name> sparse-checkout set --no-cone \
  '/*' \
  '!/.insomnia/' \
  '!/.insomnia/**'
```

既に branch がある場合は `-b` で新規作成せず、既存 branch を使うか人間へ確認する。`git worktree add` が失敗した場合は、worktree / branch / lock の状態を確認してから人間へ報告する。

## 子 Pod へ渡す scope

子 Pod を使う場合、子 Pod の cwd は現状 main workspace のままになる。子 Pod には作業対象が child worktree であることを明示し、Bash 実行時は必ず `cd .worktree/<task-name> && ...` させる。

推奨 scope:

- read: main workspace 全体
- write: child worktree の必要最小ディレクトリだけ

例:

```text
read:  <repo>
write: <repo>/.worktree/<task-name>/crates/tui
```

子 Pod には `tickets/` や inbox を書かせない。review artifact や完了候補の記録は親 Pod が main workspace 側に書く。

## 実装中のルール

- child worktree 内で build / test / format を実行する。
- main workspace の `.insomnia` や memory は child worktree にコピーしない。
- `.insomnia` が child worktree に現れた場合は作業を止め、sparse checkout 設定を確認する。
- ticket 要件外の設計変更、scope / permission / history 永続化 / prompt context 加工原則に触れる変更は実装前に人間へ確認する。
- 依存関係追加や大きな設計変更が必要になった場合も人間へ確認する。

## 完了時

実装が終わったら、merge は行わず、以下を報告する。

- worktree path
- branch 名
- 変更ファイル
- 実装概要
- 実行した build / test / format
- 未解決事項
- review に回せるかどうか

マージウィンドウとして人間が明示的にこの Workflow を呼んだ場合だけ、merge / worktree remove / branch cleanup の候補手順を提示する。実行は人間の明示許可を待つ。
