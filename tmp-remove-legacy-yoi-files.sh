#!/usr/bin/env sh
set -eu

# Temporary cleanup script for files that should stop being used after the
# workspace DB/runtime-config migration.
#
# Preconditions before running:
#   1. The backend/server process is stopped.
#   2. `[[runtimes.remote]]` from `.yoi/workspace-backend.local.toml` has been
#      copied to the resolved config-dir `runtimes.toml`, e.g.
#      `$XDG_CONFIG_HOME/yoi/runtimes.toml` when no YOI_CONFIG_DIR/YOI_HOME is set.
#   3. Repository records from `.yoi/workspace-backend.local.toml` have been
#      imported into the workspace control-plane DB.
#   4. Legacy `.yoi/tickets` has been imported with `yoi ticket import-local`.
#   5. `.yoi/workspace.db` has been copied/moved to the resolved XDG data path.
#
# This intentionally does NOT remove:
#   - `.yoi/workspace.toml`
#   - `.yoi/workflow/**`
#
# Run explicitly with:
#   RUN_YOI_LEGACY_CLEANUP=1 sh ./tmp-remove-legacy-yoi-files.sh

if [ "${RUN_YOI_LEGACY_CLEANUP:-}" != "1" ]; then
  echo "Refusing to remove files. Set RUN_YOI_LEGACY_CLEANUP=1 after completing migration preconditions." >&2
  exit 1
fi

if [ ! -f ".yoi/workspace.toml" ]; then
  echo "Refusing to run outside a Yoi workspace root: .yoi/workspace.toml not found." >&2
  exit 1
fi

rm -rf \
  .yoi/tickets

rm -f \
  .yoi/workspace-backend.local.toml \
  .yoi/workspace.db \
  .yoi/workspace.db-shm \
  .yoi/workspace.db-wal

echo "Removed legacy workspace-local backend config, Ticket files, and workspace DB files."
