# Ticket CLI instructions drift

`AGENTS.md` の Ticket 手順と現在の `yoi ticket` CLI / backend が一致していない。

- 手順は `yoi ticket create --title "..." --priority P2` を案内するが、実CLIは
  `--priority` を受け付けない。
- 手順は `.yoi/tickets/<id>/` のflat fileをauthorityとして説明するが、実CLIは
  workspace SQLite DBをauthorityとしており、作成後にgit管理対象のTicket fileは
  生成されない。
- そのため「Ticketを作成・詳細化してcommitしてからbranchを切る」という手順を、
  現在のCLIだけでは実行できない。

原因はCLIの不足ではなく、Yoi system instructions / typed Ticket tools が所有すべき
操作手順を、リポジトリ固有の `AGENTS.md` が古い具体例として重複所有していたことに
ある。

対応として、汎用のTicket運用原則をYoiのsystem promptへ移し、`AGENTS.md`には
ドッグフーディング時の境界だけを残す。typed Ticket toolsを持たないCodex等の
クライアントは、CLIやbackend直接操作で代替せず、Ticket操作をYoi Workerへ委ねる。
