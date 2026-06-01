```
gap          |
task(if some)| 8 tasks - pending: 2, inprogress:1, completed:5
             |-----------------------------------------------------------
status       |● yoi idle                          42.1k / 200k (21%)
input        |> 
actionbar    |                                        ↑ scrolled [normal]
```

status 右端は常に session context usage を `<tokens> / <window> (<pct>%)` 形式で表示する。mode / scrolled などの操作状態は actionbar に寄せる。
