local p = yoi.profile.import("builtin:default")

p.slug = "intake"
p.description = "Intake role profile with bundled reusable policy"
p.worker.instruction = "$yoi/role/intake"
p.feature = {
    task = { enabled = false },
    memory = { enabled = true },
    web = { enabled = true },
    workers = { enabled = false },
    ticket = { enabled = true, access = "lifecycle" },
    ticket_orchestration = { enabled = false },
}

return p
