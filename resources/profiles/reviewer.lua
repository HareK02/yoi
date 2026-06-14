local p = yoi.profile.import("builtin:default")

p.slug = "reviewer"
p.description = "Reviewer role profile with bundled reusable policy"
p.scope = yoi.scope.workspace_read()
p.worker.instruction = "$yoi/role/reviewer"
p.feature = {
    task = { enabled = false },
    memory = { enabled = true },
    web = { enabled = true },
    pods = { enabled = false },
    ticket = { enabled = false, access = "lifecycle" },
    ticket_orchestration = { enabled = false },
}

return p
