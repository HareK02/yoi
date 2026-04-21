---
name: Explicit out-of-scope violation is Request-changes
description: Precedent — when implementation contradicts ticket's 要件 or 範囲外 sections verbatim, reviewer should Request changes even if the change seems like a quality-of-life improvement
type: feedback
---

実装が ticket の「要件」または「範囲外」に明記された指示と矛盾している場合、それが「どう見ても spirit としては改善」であっても Request changes が相場。

**Why:** チケットのライフサイクルは CLAUDE.md で "b. 詳細化や前提の変化: `tickets/foo.md` を更新して commit" と定められており、仕様判断は ticket 更新で先に通すのが手順。実装から暗黙に上書きすると、後続チケットが前提にしている文面とのズレが残る。過去の「protocol-tool-result-shape」では要件に「`output` は残す」、範囲外に「`ToolOutput.content` の protocol 化は別チケット」と明記されているにもかかわらず protocol フィールドを `output: String` → `content: Option<String>` に改名する diff が出て、Blocking として指摘。

**How to apply:**
- 要件・範囲外セクションの明示的な文言に反する変更を見つけたら、たとえ技術的には合理的でも Blocking として扱う
- 対処方針として (1) 元通りに戻す、(2) ticket を先に書き換える、の 2 択を review に書き添える
- 「内部モデル (worker 側 struct など) と名前を揃える」系の動機は特に見落としやすい。protocol / wire 層は内部型と分離しておくのが既定、合わせるなら意思決定を ticket に上げさせる
