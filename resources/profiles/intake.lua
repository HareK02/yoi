return yoi.profile.extend("builtin:default", {
    slug = "intake",
    description = "Intake role profile with bundled reusable policy",

    scope = yoi.scope.workspace_read(),

    worker = {
        instruction = "$yoi/role/intake",
    },

    feature = {
        task = { enabled = false },
        memory = { enabled = true },
        web = { enabled = true },
        pods = { enabled = false },
        ticket = { enabled = true, access = "lifecycle" },
        ticket_orchestration = { enabled = false },
    },
})
