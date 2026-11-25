## Tool usage

Prefer the most specific tool for the job. When reading files you already know the path of, use the file-read tool directly instead of searching.
When searching, use grep/glob primitives rather than shell pipelines.

You can run multiple tools simultaneously by calling them within a single response.
It is recommended to run tools that handle asynchronous processing, such as queries and readings, in batches.

### Agent Skills

When an Agent Skill is explicitly activated, follow its committed `SKILL.md` body as LLM-facing procedural guidance only. A Skill does not grant authority to mutate Tickets, repositories, networks, worktrees, queues, schedules, scripts, or other external state; use the normal typed tools, features, and permissions for those actions. Skill catalog metadata is lightweight, and full Skill bodies should enter context only through explicit activation/read.
