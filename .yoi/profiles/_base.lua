local profile = require("yoi.profile")
local scope = require("yoi.scope")
local compact = require("yoi.compact")

return function(opts)
    return profile {
        slug = opts.slug,
        description = opts.description,

        scope = opts.scope or scope.workspace_read(),

        session = {
            record_event_trace = true,
        },

        worker = {
            reasoning = "high",
            language = opts.language or "Japanese",
        },

        model = {
            ref = opts.model_ref,
        },

        compaction = compact.tokens {
            threshold = 240000,
            request_threshold = 270000,
            worker_context_max_tokens = 100000,
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
end
