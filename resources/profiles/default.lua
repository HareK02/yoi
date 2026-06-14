return yoi.profile {
    slug = "default",
    description = "Default Yoi coding profile",


    session = {
        record_event_trace = true,
    },

    worker = {
        reasoning = "high",
    },

    model = yoi.models.catalog("codex-oauth/gpt-5.5"),

    compaction = yoi.compact.tokens {
        threshold = 240000,
        request_threshold = 270000,
        worker_context_max_tokens = 100000,
    },

    feature = {
        task = { enabled = true },
        memory = { enabled = true },
        web = { enabled = true },
        pods = { enabled = true },
        ticket = { enabled = false, access = "lifecycle" },
        ticket_orchestration = { enabled = false },
    },

    memory = {
        extract_threshold = 50000,
        consolidation_threshold_files = 5,
        consolidation_threshold_bytes = 50000,
    },

    web = {
        enabled = true,
        search = {
            provider = "brave",
            api_key_secret = "web/brave/default",
        },
    },
}
