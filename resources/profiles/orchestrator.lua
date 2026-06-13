return yoi.profile.extend("builtin:default", {
    slug = "orchestrator",
    description = "Orchestrator role profile with bundled reusable policy",

    scope = "workspace_read",

    worker = {
        instruction = "$yoi/role/orchestrator",
    },

    feature = {
        task = { enabled = false },
        memory = { enabled = true },
        web = { enabled = true },
        pods = { enabled = true },
        ticket = { enabled = true, access = "lifecycle" },
        ticket_orchestration = { enabled = true },
    },

    delegation_scope = "workspace_write",
})
