local base = require("_base")

return base {
    slug = "intake",
    description = "Intake role profile: GPT-5.5 with bundled default behavior",
    model_ref = "codex-oauth/gpt-5.5",
    feature = {
        task = { enabled = true },
        memory = { enabled = true },
        web = { enabled = true },
        pods = { enabled = false },
        ticket = { enabled = true, access = "lifecycle" },
        ticket_orchestration = { enabled = false },
    },
    language = "Japanese",
}
