return yoi.profile.extend("builtin:default", {
    slug = "orchestrator",
    description = "Orchestrator role profile with bundled reusable policy",

    scope = yoi.scope.workspace_read(),

    feature = {
        task = { enabled = false },
        memory = { enabled = true },
        web = { enabled = true },
        pods = { enabled = true },
        ticket = { enabled = true, access = "lifecycle" },
        ticket_orchestration = { enabled = true },
    },

    delegation_scope = yoi.scope.workspace_write(),
})
