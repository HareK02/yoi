local p = yoi.profile.import("builtin:default")

p.slug = "orchestrator"
p.description = "Orchestrator role profile with bundled reusable policy"
p.worker.instruction = "$yoi/role/orchestrator"
p.feature = {
    task = { enabled = false },
    memory = { enabled = true },
    web = { enabled = true },
    workers = { enabled = true },
    ticket = { enabled = true, access = "lifecycle" },
    ticket_orchestration = { enabled = true },
}

return p
