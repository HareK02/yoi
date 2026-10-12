# memory

Defines shared Memory schemas, extraction candidates, evidence, and typed communication.
Does not own persistence or implicitly convert between legacy single-document Workspace Memory and per-subject Memory.

## Main entry points

- [`backend`]: legacy Workspace Memory operation contracts and resident responses
- [`extract`]: candidate extraction and progress pointers
- [`schema`]: shared document and evidence types
- [`audit`]: audit-event transport types
