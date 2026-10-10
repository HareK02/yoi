# SubWorker cwd と複数 subtree 分担の障壁

T-739 実装時の観測。SubWorkerSpawn の説明では `cwd` は default tool cwd であり、Workdir root や capability を変えない。しかし次の child では、ファイルツールの attachment が cwd subtree を root として扱い、明示した sibling write scope を利用できないと報告された。

- `ticket-consumers`: cwd `crates/worker`。worker、client、yoi、manifest の対象ファイルに限定した write scope を委譲したが、通常 Read/Edit の logical path は worker 配下に限定された。committed session の報告 `E01a12680-0113-7552-a54b-f7fabc04506c`。
- `ticket-api`: cwd `crates/server-api`。server-api、web、resources/prompts を明示的に委譲したが、Web/Prompt/生成出力は attachment 外として未変更。報告 `E01a1267f-7506-7a43-b61e-2cacbe092834`。

親は拒否された child に別経路で書かせず、境界内の部分結果だけを確認した。解放後、Client/CLI/Web は個別 cwd と明示 scope の別 child に分け、Prompt/生成同期は親が担当した。予想より細かい分担と再統合が必要になった。

改善案: Spawn の documented cwd 契約と file tool logical-root の解決を照合する。cwd が attachment を狭める仕様なら spawn 前に sibling scope を明示的に拒否し、実効 logical root と利用可能 subtree を返す。default cwd と logical root が別なら、その差を Read/Edit のエラーおよび初期 context に表示する。scope 拒否を回避する仕組みの要望ではなく、委譲時に有効な分担を予測できる診断の要望である。
