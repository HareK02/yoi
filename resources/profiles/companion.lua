local p = yoi.profile.import("builtin:default")

p.slug = "companion"
p.description = "Companion role profile with bundled reusable policy"
p.feature = {
    task = { enabled = true },
    memory = { enabled = true },
    web = { enabled = true },
    pods = { enabled = true },
    ticket = { enabled = true, access = "lifecycle" },
    ticket_orchestration = { enabled = false },
}

return p
