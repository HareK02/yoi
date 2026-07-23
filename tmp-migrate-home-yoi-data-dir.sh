#!/usr/bin/env sh
set -eu

# Move the legacy home data root (`$HOME/.yoi`) to the current Yoi data dir.
#
# IMPORTANT:
#   This moves the directory that currently contains live sessions, worker
#   runtime state, workdirs, and this checkout's parent path. Do not run while
#   Yoi backend/runtime/workers/TUI/web sessions are running.
#
# Recommended usage after shutdown:
#   cd /tmp
#   RUN_YOI_HOME_MIGRATION=1 sh /path/to/tmp-migrate-home-yoi-data-dir.sh
#
# Destination resolution matches manifest::paths::data_dir():
#   1. YOI_DATA_DIR
#   2. YOI_HOME
#   3. XDG_DATA_HOME/yoi
#   4. HOME/.local/share/yoi
#
# This script intentionally refuses to merge into a non-empty destination. If
# the destination already has data, inspect and merge manually.

if [ "${RUN_YOI_HOME_MIGRATION:-}" != "1" ]; then
  echo "Refusing to migrate. Set RUN_YOI_HOME_MIGRATION=1 after stopping all Yoi processes." >&2
  exit 1
fi

HOME_DIR=${HOME:?HOME is required}
OLD_DIR=${OLD_YOI_DATA_DIR:-"$HOME_DIR/.yoi"}

if [ -n "${YOI_DATA_DIR:-}" ]; then
  NEW_DIR=$YOI_DATA_DIR
elif [ -n "${YOI_HOME:-}" ]; then
  NEW_DIR=$YOI_HOME
elif [ -n "${XDG_DATA_HOME:-}" ]; then
  NEW_DIR=$XDG_DATA_HOME/yoi
else
  NEW_DIR=$HOME_DIR/.local/share/yoi
fi

if [ ! -d "$OLD_DIR" ]; then
  echo "Legacy data dir does not exist: $OLD_DIR" >&2
  exit 1
fi

case "$NEW_DIR" in
  "$OLD_DIR"|"$OLD_DIR"/*)
    echo "Refusing to move into the old data dir or one of its children: $NEW_DIR" >&2
    exit 1
    ;;
esac

CURRENT_DIR=$(pwd -P)
case "$CURRENT_DIR" in
  "$OLD_DIR"|"$OLD_DIR"/*)
    echo "Refusing to run from inside $OLD_DIR. cd outside it first, e.g. cd /tmp." >&2
    exit 1
    ;;
esac

if [ -e "$NEW_DIR" ]; then
  if [ -d "$NEW_DIR" ] && [ -z "$(find "$NEW_DIR" -mindepth 1 -maxdepth 1 -print -quit)" ]; then
    rmdir "$NEW_DIR"
  else
    echo "Destination already exists and is not empty: $NEW_DIR" >&2
    echo "Inspect and merge manually; this script will not overwrite or merge data." >&2
    exit 1
  fi
fi

mkdir -p "$(dirname "$NEW_DIR")"
mv "$OLD_DIR" "$NEW_DIR"

echo "Moved legacy Yoi data dir:"
echo "  from: $OLD_DIR"
echo "  to:   $NEW_DIR"
