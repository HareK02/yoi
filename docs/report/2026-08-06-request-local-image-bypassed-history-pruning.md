# Request-local image injection bypassed durable history and pruning

Date: 2026-08-06

## Symptom

The first `ViewImage` implementation stored image bytes in `ToolOutput.attachments` with
`#[serde(skip)]`, projected them into a synthetic user image only while building one provider
request, and then cleared the attachment from Worker history. The model could therefore answer
from pixels that disappeared from the next turn, session restore, pruning, and compaction
observation.

## Why this was wrong

Yoi's context policy requires new model-visible input to be appended and committed to
`worker.history` before request construction. Pure pruning may alter a request-context clone
because the projection is deterministic from durable history; request-local input injection is
not equivalent.

The transient image also changed the middle of the next prompt prefix without an explicit prune,
which reduced prompt-cache reuse and left later turns without the evidence behind the assistant
response.

Local reference review confirmed the intended pattern:

- Codex records `view_image` as image-bearing `FunctionCallOutput` content and explicitly tests
  that no separate image message is injected.
- OpenCode persists image file parts in Session state and makes media removal an explicit
  compaction/pruning decision.

## Resolution

Image attachments are now durable ToolResult detail:

- Image bytes are base64-serialized through normal Item/session-log persistence.
- OpenAI Responses lowers them to `function_call_output` content items.
- OpenAI Chat deterministically lowers the durable ToolResult to tool text followed by a user
  image message while preserving parallel-tool-result ordering.
- The existing ToolResult pruning projection removes both text detail and image attachments from
  the request-context clone while retaining the summary and original persistent history.
- Session exploration and compaction-facing projections expose only bounded attachment metadata,
  never raw base64 as ordinary text.

## Guardrail

Do not use `#[serde(skip)]`, post-build clearing, or one-shot synthetic messages for input that can
influence model output. Provider-specific synthetic messages are acceptable only when they are a
stable projection of a committed history item and are regenerated identically until an explicit
pruning or compaction boundary.
