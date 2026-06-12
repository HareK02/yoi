return yoi.profile.extend("builtin:default", {
    slug = "reviewer",
    description = "Reviewer role profile with bundled reusable policy",

    scope = yoi.scope.workspace_read(),

    worker = {
        instruction = "$yoi/role/reviewer",
    },

    feature = {
        task = { enabled = false },
        memory = { enabled = true },
        web = { enabled = true },
        pods = { enabled = false },
        ticket = { enabled = false, access = "lifecycle" },
        ticket_orchestration = { enabled = false },
    },
})
