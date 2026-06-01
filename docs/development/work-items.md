# Work items

Yoi project work is tracked through `work-items/` and `./tickets.sh`. Git history plus work item files are the authoritative state-transition record.

Do not treat ad-hoc chat summaries, memory records, or Pod notifications as the final source of project state.

## Basic commands

```sh
./tickets.sh create --title "..." [--slug slug] [--kind task] [--priority P2] [--label a,b]
./tickets.sh list [--status open|pending|closed|all]
./tickets.sh show <id-or-slug>
./tickets.sh comment <id-or-slug> [--role comment|plan|decision|implementation_report] [--file path]
./tickets.sh review <id-or-slug> --approve|--request-changes [--file path]
./tickets.sh status <id-or-slug> open|pending|closed
./tickets.sh close <id-or-slug> [--resolution text|--file path]
./tickets.sh doctor
```

## Granularity

One work item should describe a complete change that can be explained as a feature, behavior, design decision, or maintenance outcome when closed.

Avoid tickets that only mirror an implementation step unless that step is independently reviewable and useful. Phase/step lists inside a ticket are execution order, not a separate dependency system.

## Contents

A useful work item states:

- background and motivation
- requirements
- acceptance criteria
- relevant constraints
- review/implementation reports when work is submitted
- final resolution when closed

Keep long research dumps out of the item body. Put necessary artifacts under the ticket's `artifacts/` directory and summarize the conclusion in the thread.

## Workflow

Create or refine the work item before opening a separate implementation worktree. When using child Pods, the orchestrator should provide a scoped task and later verify concrete evidence: diff, tests, worktree status, and child output.

Closing a ticket means the repository records are ready, not merely that a child Pod announced completion.
