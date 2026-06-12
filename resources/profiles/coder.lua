return yoi.profile.extend("builtin:default", {
    slug = "coder",
    description = "Coder role profile with bundled reusable policy",

    scope = yoi.scope.workspace_write(),

    feature = {
        task = { enabled = true },
        memory = { enabled = true },
        web = { enabled = true },
        pods = { enabled = false },
        ticket = { enabled = false, access = "lifecycle" },
        ticket_orchestration = { enabled = false },
    },
})
