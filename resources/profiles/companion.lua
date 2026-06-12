return yoi.profile.extend("builtin:default", {
    slug = "companion",
    description = "Companion role profile with bundled reusable policy",

    scope = yoi.scope.workspace_write({
        deny_write = { ".worktree" },
    }),

    feature = {
        task = { enabled = true },
        memory = { enabled = true },
        web = { enabled = true },
        pods = { enabled = true },
        ticket = { enabled = true, access = "lifecycle" },
        ticket_orchestration = { enabled = false },
    },
})
