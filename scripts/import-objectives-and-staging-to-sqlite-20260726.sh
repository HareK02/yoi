#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/import-objectives-and-staging-to-sqlite-20260726.sh [--workspace PATH] [--database PATH] [--apply]

Imports legacy .yoi/objectives records and .yoi/memory/_staging JSON files into
Workspace SQLite authority by invoking yoi-workspace-server import-objectives.

By default this is a dry-run of the command line only. Pass --apply to execute.
EOF
}

workspace="$(pwd)"
database="${YOI_SERVER_DB:-$HOME/.local/share/yoi/server/server.db}"
apply=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --workspace)
      workspace="$2"
      shift 2
      ;;
    --workspace=*)
      workspace="${1#--workspace=}"
      shift
      ;;
    --database)
      database="$2"
      shift 2
      ;;
    --database=*)
      database="${1#--database=}"
      shift
      ;;
    --apply)
      apply=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

cmd=(cargo run -q -p yoi-workspace-server -- import-objectives --workspace "$workspace" --database "$database")

printf 'Workspace: %s\n' "$workspace"
printf 'Database:  %s\n' "$database"
printf 'Command:   '
printf '%q ' "${cmd[@]}"
printf '\n'

if [[ "$apply" -ne 1 ]]; then
  echo 'Dry run only. Re-run with --apply to import.'
  exit 0
fi

"${cmd[@]}"
