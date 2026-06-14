local p = yoi.profile.import("builtin:default")

p.slug = "coder"
p.description = "Coder role profile with bundled reusable policy"
p.scope = yoi.scope.workspace_write()
p.worker.instruction = "$yoi/role/coder"
p.feature = {
    task = { enabled = true },
    memory = { enabled = true },
    web = { enabled = true },
    pods = { enabled = false },
    ticket = { enabled = false, access = "lifecycle" },
    ticket_orchestration = { enabled = false },
}

return p
