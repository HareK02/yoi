<!-- event: create author: tickets.sh at: 2026-05-30T05:49:27Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-05-30T05:50:04Z -->

## Plan

## Preflight

Classification: research-first / implementation-ready after sources are recorded.

The work is mostly data/catalog maintenance. It should begin with current provider documentation/model-list research and a short source note before editing the catalog. Implementation should be limited to `resources/models/builtin.toml` and directly related docs/tests unless research proves a provider definition is wrong.

Critical risks:
- Do not guess model IDs or context windows from memory.
- Do not add models that the current provider client cannot address.
- Do not churn provider definitions unless needed.
- If changing the default profile model, explain the product reason and verify compaction/effective window metadata.


---

<!-- event: decision author: hare at: 2026-05-30T23:13:31Z -->

## Decision

Research note for builtin catalog refresh:

Sources checked:

- Anthropic Models overview (`https://docs.anthropic.com/en/docs/about-claude/models/overview`, redirected to `https://platform.claude.com/docs/en/about-claude/models/overview`): current comparison lists Claude Opus 4.8, Claude Sonnet 4.6, and Claude Haiku 4.5. API IDs: `claude-opus-4-8`, `claude-sonnet-4-6`, `claude-haiku-4-5-20251001`; aliases include `claude-haiku-4-5`. Context windows: Opus 4.8 1M, Sonnet 4.6 1M, Haiku 4.5 200k. Opus 4.8 is described as the starting point for most complex tasks, but the table says Extended thinking: No, so the catalog gives it an explicit capability without `reasoning = "budget_tokens"`.
- OpenAI Models overview (`https://platform.openai.com/docs/models`, redirected to `https://developers.openai.com/api/docs/models`): recommends `gpt-5.5` for complex reasoning/coding, with `gpt-5.4` and `gpt-5.4-mini` as lower latency/cost variants. `gpt-5.5` and `gpt-5.4` have 1.05M context windows and 128k max output.
- OpenAI model detail pages:
  - `https://developers.openai.com/api/docs/models/gpt-5.5`: model ID `gpt-5.5`, 1,050,000 context window, xhigh reasoning support, notes prompts over 272K input tokens are charged differently; local catalog retains `max_context_window = 272000` for the existing backend/effective-window clamp decision.
  - `https://developers.openai.com/api/docs/models/gpt-5.4`: model ID `gpt-5.4`, 1,050,000 context window.
  - `https://developers.openai.com/api/docs/models/gpt-5-codex`: model ID `gpt-5-codex`, 400,000 context window, Responses API only, optimized for agentic coding in Codex/similar environments.
- OpenRouter model list endpoint (`https://openrouter.ai/api/v1/models`): confirmed `anthropic/claude-opus-4.8` (1M), `anthropic/claude-sonnet-4.6` (1M), and `openai/gpt-5.5` (1.05M) with tools/structured output/reasoning parameters. Dynamic `~...latest` router aliases exist, but the builtin catalog uses concrete IDs to avoid unstable default behavior.
- Ollama Library:
  - `https://ollama.com/library/llama3.3`: `llama3.3` latest/70b has 128K context.
  - `https://ollama.com/library/qwen3-coder`: `qwen3-coder` latest/30b has 256K context and is positioned for agentic/coding tasks.

Selected changes:

- Anthropic direct: replace stale `claude-sonnet-4-5` / `claude-opus-4-1` with `claude-opus-4-8`, `claude-sonnet-4-6`, and `claude-haiku-4-5`; update Sonnet context to 1M.
- Codex OAuth/OpenAI: keep default `codex-oauth/gpt-5.5`, update advertised context to 1.05M while retaining the existing 272K effective clamp; replace older plain `gpt-5` entry with `gpt-5.4`; keep `gpt-5-codex` because OpenAI documents it as a Codex/similar-environment Responses model.
- OpenRouter: replace stale `anthropic/claude-sonnet-4` / `openai/gpt-5` with concrete current IDs `anthropic/claude-opus-4.8`, `anthropic/claude-sonnet-4.6`, and `openai/gpt-5.5`.
- Ollama: replace `llama3.1` / `qwen2.5-coder` with current generic local placeholders `llama3.3` and `qwen3-coder`.
- Provider definitions unchanged; no provider-level source indicated that `resources/providers/builtin.toml` is stale.
- Default profile remains `codex-oauth/gpt-5.5`; this remains aligned with OpenAI’s current model recommendation and the existing effective-context clamp used by compaction safety.


---

<!-- event: review author: hare at: 2026-05-30T23:16:22Z status: approve -->

## Review: approve

External review by reviewer Pod `builtin-catalog-reviewer-20260530`: approve.

Reviewer summary:

- Sources were recorded in the ticket thread before/with the catalog changes.
- Catalog changes align with the recorded sources:
  - Anthropic entries now use Opus 4.8 / Sonnet 4.6 / Haiku 4.5, with Sonnet and Opus 1M context and Haiku 200k.
  - Codex OAuth/OpenAI entries use `gpt-5.5`, `gpt-5.4`, and `gpt-5-codex`; `gpt-5.5` retains the existing `max_context_window = 272000` effective clamp while advertising 1.05M.
  - OpenRouter uses concrete current IDs, avoiding unstable `~...latest` aliases.
  - Ollama entries use `llama3.3` and `qwen3-coder`.
- Provider definitions and default profile were reasonably left unchanged.
- Reported validation was adequate.

Blockers: none.

Non-blocking note addressed after review:
- The provider catalog test name/assertion still implied provider-default capability fallback for a model that now has explicit capability. It was renamed/updated to describe provider+model catalog merge semantics.


---

<!-- event: implementation_report author: hare at: 2026-05-30T23:17:25Z -->

## Implementation report

Implementation report:

- Refreshed `resources/models/builtin.toml` from recorded official/semiofficial sources.
- Anthropic direct entries now cover `claude-opus-4-8`, `claude-sonnet-4-6`, and `claude-haiku-4-5`; Sonnet/Opus context windows are 1M and Haiku is 200k. Opus has explicit capability without `reasoning = "budget_tokens"` because the source table says Extended thinking is not supported.
- Ollama local placeholders are now `llama3.3` (128K) and `qwen3-coder` (256K).
- Codex OAuth/OpenAI entries now cover `gpt-5.5`, `gpt-5.4`, and `gpt-5-codex`; `gpt-5.5` advertises 1.05M while retaining the existing `max_context_window = 272000` effective clamp.
- OpenRouter entries now use concrete current IDs `anthropic/claude-opus-4.8`, `anthropic/claude-sonnet-4.6`, and `openai/gpt-5.5`; dynamic latest aliases were intentionally not added.
- `resources/providers/builtin.toml` and `resources/profiles/default.lua` were left unchanged.
- Updated provider catalog test expectations and renamed the affected test to avoid implying provider-default capability fallback for an explicitly cataloged model.

External review:

- Reviewer Pod `builtin-catalog-reviewer-20260530` approved with no blockers.
- Reviewer non-blocking note about the stale test name/assert message was addressed.

Validation:

- `cargo fmt --check` passed
- `cargo test -p provider` passed
- `cargo test -p manifest model` passed
- `cargo test -p manifest profile -- --nocapture` passed
- `cargo check -p provider -p manifest` passed
- `./tickets.sh doctor` passed
- `git diff --check` passed


---

<!-- event: close author: hare at: 2026-05-30T23:17:38Z status: closed -->

## Closed

Refreshed the builtin model catalog from recorded official/semiofficial sources. Anthropic, OpenAI/Codex OAuth, OpenRouter, and Ollama entries now point at current concrete model IDs; default profile remains ; provider definitions were unchanged. External review approved and validation passed: cargo fmt --check, cargo test -p provider, cargo test -p manifest model, cargo test -p manifest profile, cargo check -p provider -p manifest, ./tickets.sh doctor, git diff --check.


---

<!-- event: close author: hare at: 2026-05-30T23:17:46Z status: closed -->

## Closed

Refreshed the builtin model catalog from recorded official/semiofficial sources. Anthropic, OpenAI/Codex OAuth, OpenRouter, and Ollama entries now point at current concrete model IDs; default profile remains `codex-oauth/gpt-5.5`; provider definitions were unchanged.

External review approved and validation passed:

- `cargo fmt --check`
- `cargo test -p provider`
- `cargo test -p manifest model`
- `cargo test -p manifest profile -- --nocapture`
- `cargo check -p provider -p manifest`
- `./tickets.sh doctor`
- `git diff --check`


---
