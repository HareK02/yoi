local base = require("_base")

return base {
    slug = "reviewer",
    description = "Reviewer role profile: GPT-5.5 with bundled default behavior",
    model_ref = "codex-oauth/gpt-5.5",
    feature = {
        task = { enabled = false },
        memory = { enabled = true },
        web = { enabled = true },
        pods = { enabled = false },
        ticket = { enabled = false, access = "lifecycle" },
        ticket_orchestration = { enabled = false },
    },
    language = "Japanese",
}
