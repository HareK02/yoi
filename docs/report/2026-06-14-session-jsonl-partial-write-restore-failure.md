# Session JSONL partial write caused restore failure

## Summary

`yoi-orchestrator` restore failed with a misleading Pod I/O error:

```text
failed to restore pod yoi-orchestrator
I/O error: stream did not contain valid UTF-8
```

The error was not caused by Panel, SendToPod, Unix socket IPC, or the LLM SSE stream. The active session JSONL file contained a corrupted line with invalid UTF-8 and invalid JSONL structure, so restore failed while reading persisted history before any model run could start.

## Observed symptoms

- Sending to `yoi-orchestrator` failed from multiple client paths with the same notice error.
- Restore also failed with the same UTF-8 I/O error.
- Enabling trace did not produce new transport trace entries for this failure, because restore failed before LLM transport execution.
- Raw socket checks showed the Pod socket/snapshot path itself was not the original source of the invalid UTF-8 error.

## Corruption found

The corrupted file was the active `yoi-orchestrator` session JSONL under `~/.yoi/sessions/...`.

The corrupted record was a Bash `tool_result` containing truncated-output text similar to:

```text
[showing last 80 of 311 lines ...
```

The line contained an incomplete UTF-8 sequence in the truncated-output header, likely the first two bytes of an em dash (`e2 80`, missing the final byte), followed immediately by the next JSON record on the same line. This made the file both invalid UTF-8 and invalid JSONL.

The corrupted `tool_result` was manually replaced with a synthetic repair record preserving the original `call_id`, and the following assistant record was split back onto its own line. The repaired file was validated as UTF-8 and JSONL.

## Impact

- A single corrupted append in session history can make a Pod unrestorable.
- The current error surfaced as a generic I/O/UTF-8 error and did not name the session file, line, byte offset, or restore phase.
- Because the same phrase can also appear in transport/SSE decoding failures, the diagnosis path was initially confusing.

## Lessons / improvement ideas

- Session restore errors should include file path, line number, byte offset if available, and phase (`history restore`, `trace read`, `transport stream`, etc.).
- JSONL append should avoid leaving a partially written record followed by later records on the same line. Consider atomic record append safeguards, newline recovery, or corruption quarantine.
- Restore could offer a bounded repair/quarantine mode for malformed trailing or individual records, especially tool results.
- Bash truncated-output serialization should avoid multi-byte punctuation in structural prefixes or ensure all persisted records are validated before commit.
- Transport SSE UTF-8 failures and session-history UTF-8 failures should have clearly distinct error wording.

## Related fixes made during investigation

- Added safer SSE parse diagnostics in `llm-worker` so future provider-stream failures include HTTP status and selected safe response headers.
- Enabled local trace via `.yoi/override.local.toml` and manually set `record_event_trace = true` in the `yoi-orchestrator` metadata snapshot for future restores.
