local base = require("_base")
local scope = require("yoi.scope")

return base {
    slug = "orchestrator",
    description = "Orchestrator role profile: GPT-5.5 with bundled default behavior",
    delegation_scope = scope.workspace_write(),
    feature = {
        task = { enabled = true },
        memory = { enabled = true },
        web = { enabled = true },
        pods = { enabled = true },
        ticket = { enabled = true, access = "lifecycle" },
        ticket_orchestration = { enabled = true },
    },
    model_ref = "codex-oauth/gpt-5.5",
    language = "Japanese",
}
