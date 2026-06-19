Closed as completed.

Added E2E coverage for a Panel shell-enter launch path:
- PTY starts `/bin/sh` and sends `exec <yoi> panel ...` as the command line;
- measurement starts immediately before sending the command line / Enter-equivalent input;
- the test waits for the existing dashboard content-ready snapshot rather than first frame or a single row;
- isolated HOME/XDG/runtime fixture remains in place and `YOI_POD_RUNTIME_COMMAND` is pinned to the tested binary;
- direct-spawn Panel readiness tests remain as separate coverage.

Validation was recorded during implementation, including the focused shell-enter test, the full Panel E2E test set, relevant cargo check, formatting, diff check, ticket doctor, and Nix build.
