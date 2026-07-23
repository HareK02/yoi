#!/usr/bin/env sh
set -eu

# Migration helper for workspace-local runtime config and legacy Ticket files.
#
# It performs:
#   1. Extract `[[runtimes.remote]]` blocks from `.yoi/workspace-backend.local.toml`.
#   2. Write them to the resolved config-dir `runtimes.toml`.
#   3. Remove those runtime blocks from `.yoi/workspace-backend.local.toml` so the
#      updated backend no longer rejects the workspace config.
#   4. Run `cargo run --bin yoi -- ticket import-local` to import `.yoi/tickets`
#      into the workspace SQLite DB.
#
# It intentionally does NOT delete legacy files. Run
# `tmp-remove-legacy-yoi-files.sh` only after repository DB import, ticket import,
# and DB location migration have all been verified.
#
# Preconditions:
#   - Run from the repository/workspace root.
#   - Stop Yoi backend/runtime/workers before running.
#   - Use the updated code in this worktree.
#
# Run explicitly with:
#   RUN_YOI_WORKSPACE_RUNTIME_TICKET_MIGRATION=1 sh ./tmp-migrate-workspace-runtime-and-tickets.sh

if [ "${RUN_YOI_WORKSPACE_RUNTIME_TICKET_MIGRATION:-}" != "1" ]; then
  echo "Refusing to migrate. Set RUN_YOI_WORKSPACE_RUNTIME_TICKET_MIGRATION=1 after stopping Yoi processes." >&2
  exit 1
fi

if [ ! -f ".yoi/workspace.toml" ]; then
  echo "Refusing to run outside a Yoi workspace root: .yoi/workspace.toml not found." >&2
  exit 1
fi

CONFIG_FILE=.yoi/workspace-backend.local.toml
if [ ! -f "$CONFIG_FILE" ]; then
  echo "Workspace backend local config not found: $CONFIG_FILE" >&2
  exit 1
fi

if [ -n "${YOI_CONFIG_DIR:-}" ]; then
  CONFIG_DIR=$YOI_CONFIG_DIR
elif [ -n "${YOI_HOME:-}" ]; then
  CONFIG_DIR=$YOI_HOME/config
elif [ -n "${XDG_CONFIG_HOME:-}" ]; then
  CONFIG_DIR=$XDG_CONFIG_HOME/yoi
else
  HOME_DIR=${HOME:?HOME is required when YOI_CONFIG_DIR, YOI_HOME, and XDG_CONFIG_HOME are unset}
  CONFIG_DIR=$HOME_DIR/.config/yoi
fi

RUNTIMES_TOML=$CONFIG_DIR/runtimes.toml
RUNTIME_BLOCKS=$(mktemp)
STRIPPED_CONFIG=$(mktemp)
trap 'rm -f "$RUNTIME_BLOCKS" "$STRIPPED_CONFIG"' EXIT

awk '
  /^\[\[runtimes\.remote\]\]/ { in_runtime=1; print; next }
  in_runtime && /^\[/ && $0 !~ /^\[\[runtimes\.remote\]\]/ { in_runtime=0 }
  in_runtime { print }
' "$CONFIG_FILE" > "$RUNTIME_BLOCKS"

if [ -s "$RUNTIME_BLOCKS" ]; then
  mkdir -p "$CONFIG_DIR"
  if [ -e "$RUNTIMES_TOML" ] && [ -s "$RUNTIMES_TOML" ]; then
    echo "Refusing to overwrite or merge existing non-empty runtimes config: $RUNTIMES_TOML" >&2
    echo "Move runtime entries manually or empty the file after inspection." >&2
    exit 1
  fi
  cp "$RUNTIME_BLOCKS" "$RUNTIMES_TOML"
  echo "Wrote runtime registrations to $RUNTIMES_TOML"

  awk '
    /^\[\[runtimes\.remote\]\]/ { skip=1; next }
    skip && /^\[/ && $0 !~ /^\[\[runtimes\.remote\]\]/ { skip=0 }
    !skip { print }
  ' "$CONFIG_FILE" > "$STRIPPED_CONFIG"
  cp "$STRIPPED_CONFIG" "$CONFIG_FILE"
  echo "Removed runtime registrations from $CONFIG_FILE"
else
  echo "No [[runtimes.remote]] blocks found in $CONFIG_FILE; leaving runtimes config unchanged."
fi

cargo run --bin yoi -- ticket import-local

echo "Imported legacy .yoi/tickets into the workspace SQLite Ticket backend."
echo "Next: start the backend once to import repositories, verify data, then run tmp-remove-legacy-yoi-files.sh if safe."
