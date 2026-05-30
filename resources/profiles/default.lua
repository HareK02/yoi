local profile = require("insomnia.profile")
local scope = require("insomnia.scope")
local compact = require("insomnia.compact")

return profile {
  slug = "default",
  description = "Bundled default Insomnia coding profile",

  scope = scope.workspace_write(),

  session = {
    record_event_trace = true,
  },

  worker = {
    reasoning = "high",
  },

  model = {
    ref = "codex-oauth/gpt-5.5",
  },

  compaction = compact.tokens {
    threshold = 200000,
    request_threshold = 240000,
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
      api_key_env = "BRAVE_SEARCH_API_KEY",
    },
  },
}
