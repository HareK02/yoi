---
name: worktree-workflow
description: "Worktreeを用いた開発フローを進める。git上の開発に置けるミクロな指示で、プロジェクトの管理に関する指示は提供されていない。"
allowed-tools: "Bash(cd *), Bash(git worktree *), Bash(mkdir *), Bash(cp *), Bash(ln *), Bash(ls *), Bash(find *)"
---

# Worktreeを用いた開発

Goal: 実装を完了させ、ブランチをマージ待ちの状態にする。

`./.worktree`にworktreeを作成します。
エージェントの1セッション=1ワークツリーとしており、ブランチ/イシュー/チケット単位で切ります。

このワークフローにおいては、ブランチはローカルで並行開発するためのマージ後削除の運用とし、Worktreeと同名のbranchを同時に作って進めます。メインのディレクトリのブランチから切るものとして扱います。

```
git worktree add .worktree/<task-name> -n <task-name>
```

## flake.nixの無効化

基本的に、CWDを変更できない場合、.envrcによる自動アクティベートは効かないので無視で構わない。
