この broad MCP integration Ticket は concrete work item としては退役し、Objective `00001KTR80WMN` に格上げした。

今後の実装 authority は以下の concrete Tickets に置く。

- `00001KTR81P9X`: `pod::feature` / Worker / ToolRegistry API を external protocol-backed capability providers に耐える形へ拡張する。
- `00001KTR82RB7`: MCP `2025-11-25` local stdio server-feature bridge を実装する。

重要な判断:

- API 拡張と MCP 実装は分離する。
- `resources/prompts` は固定で scope 外にせず、direct hidden context injection を禁止したうえで明示 tool operation の result として扱う方向にする。
- Streamable HTTP、remote auth/OAuth、MCP Registry distribution、sampling、elicitation は後続判断または fail-closed とする。

この Ticket 自体は progress-container として残さない。Objective-to-Ticket links は context であり、dependency/scheduling authority ではない。