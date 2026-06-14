local p = yoi.profile.import("builtin:default")

p.slug = "orchestrator"
p.description = "Orchestrator role profile with bundled reusable policy"
p.scope = "workspace_read"
p.worker.instruction = "$yoi/role/orchestrator"
p.feature = {
    task = { enabled = false },
    memory = { enabled = true },
    web = { enabled = true },
    pods = { enabled = true },
    ticket = { enabled = true, access = "lifecycle" },
    ticket_orchestration = { enabled = true },
}
p.delegation_scope = "workspace_write"

return p
