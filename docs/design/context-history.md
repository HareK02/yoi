# Context and history

Any input that can influence the model across a turn must be committed to history before it is placed in model context.

This rule protects both explainability and prompt-cache behavior. If the model reacts to a context-only insertion that is not in history, later turns cannot explain why the assistant made that decision, and cache anchors become harder to reason about.

## Allowed context transformations

A context transformation is acceptable when it is reproducible from durable Pod state and does not introduce new volatile facts.

Examples:

- Pruning old history according to persisted/session-derived state.
- Truncating oversized tool result content while preserving the committed result boundary.
- Adding prompt-cache anchors derived from stable context structure.
- Rendering the same materialized system prompt string after compaction.

These transformations change how context is packed, not what happened.

## Forbidden context injection

Do not insert turn-crossing information directly into context without first appending it to `worker.history`.

Forbidden examples:

- Delivering a `Notify` or `PodEvent` only as a temporary context note.
- Adding a `<system-reminder>` that explains behavior but is not persisted.
- Rewriting old messages to include new facts.
- Letting UI/controller-only state become model-visible without a committed record.

If new information should affect the model, append it to history and commit it. `history.json` / session persistence follows from the Worker history path.

## Prompt cache implications

Yoi optimizes for predictable prompt shape, not only small requests. Hidden context insertions make cache behavior hard to reproduce because the model-visible input can change without a corresponding history record.

Ordinary new inputs are not the problem: user messages, committed events, compacted summaries, and tool results are expected additions to history. The avoidable problem is changing prior or side-channel context in a way that is neither durable nor explainable from records.

## Pruning and compaction

Pruning is a request-time packing operation that chooses which existing history to include under a token budget. Compaction is a durable history operation that creates a new summarized state and must be committed.

After compaction, context safety must be revalidated. A flag such as `just_compacted` must not suppress safety checks, because compact output can itself be too large or malformed for the next request.

## UI and orchestration consequences

The TUI should display committed events rather than inventing transcript blocks. Orchestration should treat child notifications as prompts to inspect state, not as state themselves.

This keeps future turns able to answer: "what did the agent know, when did it know it, and where is that recorded?"
