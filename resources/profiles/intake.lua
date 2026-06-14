local p = yoi.profile.import("builtin:default")

p.slug = "intake"
p.description = "Intake role profile with bundled reusable policy"
p.scope = yoi.scope.workspace_read()
p.worker.instruction = "$yoi/role/intake"
p.feature = {
    task = { enabled = false },
    memory = { enabled = true },
    web = { enabled = true },
    pods = { enabled = false },
    ticket = { enabled = true, access = "lifecycle" },
    ticket_orchestration = { enabled = false },
}

return p
