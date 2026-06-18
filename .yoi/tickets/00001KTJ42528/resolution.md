Closed as resolved by current dogfooding workflow rather than a new broad implementation.

The active policy is now operationally established:
- do not commit one Ticket lifecycle event per Git commit mechanically;
- batch related Ticket-only updates when they belong to the same decision/work burst;
- keep implementation/code commits separate from Ticket publication commits unless a merge step requires both;
- stage only exact Ticket paths and never broad auto-commit unrelated user changes;
- use orchestration worktree/branch for active progress and publish to the main workspace at explicit merge/close/follow-up boundaries;
- keep audit evidence in Ticket files/thread/resolution even when Git commits are batched.

Future changes should be filed as targeted Tickets, such as Panel dirty-state UX, workflow prompt wording, or automated queue commit behavior.
