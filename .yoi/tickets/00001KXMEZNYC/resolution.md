Progress message guidance was added to `resources/prompts/common/writing.md`.

The prompt now instructs long-running work to emit short ordinary user-visible prose progress updates at meaningful boundaries, without tool calls, hidden notes, chain-of-thought, raw reasoning, secrets, or verbose tool-output details.

Validation: `nix build .#yoi` passed.
