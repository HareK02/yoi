# T-718: SubWorker の cwd・参照コンテキストに関する障壁

## 観測

T-718 の実装を `crates/ticket` / `crates/worker` / `crates/merge-request` に分担した。SubWorkerSpawn の cwd を各 crate に設定すると、子のファイル tool は `src/lib.rs` 等で成功する一方、親で使える repository-root-relative な `crates/<crate>/src/...` は crate prefix が二重になって失敗した。cwd 未指定の Glob は一部の子で transient bash-output 側のパスを logical path として扱い失敗した。

また builtin default の子には ShowTicket が公開されず、別途 read scope に含めたテスト規則の親 root-relative path も子の cwd から参照できなかった。親が authoritative Ticket とテスト規則を先に読み、要件・規則・インターフェースを本文で転送して作業を継続した。

## 改善案

- 子の startup context に実際の Workdir attachment alias・logical root・tool cwd を明示する。
- cwd と path root の関係を tool schema / Spawn 説明と一致させ、cwd 外に委譲した read-only scope の参照方法も示す。
- Ticket implementation の子へ typed Ticket read を委譲できない場合は、親に authoritative item revision と本文を渡す責任があることを明示する。

本件では委譲 scope を広げたり Backend storage を直接操作する回避はしていない。製品要件の追加ではなく、分担時の tooling/context で発生した調整コストの記録である。
