#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  scripts/migrate-workdir-layout-20260724.sh --workdir-target <PATH> [--apply]

Migrates old runtime workdir materialization layout:

  <workdir-target>/working-directories/<id>/materialization.json
  <workdir-target>/working-directories/<id>/root/<repository-id>/

to the new layout:

  <workdir-target>/<id>/materialization.json
  <workdir-target>/<id>/checkout/

The script defaults to dry-run. It mutates files only when --apply is passed.
Run this while the runtime/server and workers using the target are stopped.

Options:
  --workdir-target <PATH>  Runtime workdir materialization target.
  --apply                 Perform migration. Without this, only prints actions.
  -h, --help              Show this help.

Requirements:
  bash, git, jq, mktemp
USAGE
}

log() {
  printf '%s\n' "$*"
}

fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

json_string() {
  local file=$1
  local expr=$2
  jq -er "$expr | strings" "$file"
}

apply=0
workdir_target=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --workdir-target)
      [[ $# -ge 2 ]] || fail "--workdir-target requires a path"
      workdir_target=$2
      shift 2
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
      fail "unknown argument: $1"
      ;;
  esac
done

[[ -n "$workdir_target" ]] || fail "--workdir-target is required"
need_cmd git
need_cmd jq
need_cmd mktemp

[[ -d "$workdir_target" ]] || fail "workdir target does not exist or is not a directory: $workdir_target"
old_root=$workdir_target/working-directories
if [[ ! -d "$old_root" ]]; then
  log "no old workdir layout found: $old_root"
  exit 0
fi

if [[ $apply -eq 0 ]]; then
  log "dry-run: no files will be changed; pass --apply to migrate"
else
  log "apply: migrating old workdir layout under $workdir_target"
fi

shopt -s nullglob
records=("$old_root"/*/materialization.json)
shopt -u nullglob

if [[ ${#records[@]} -eq 0 ]]; then
  log "no materialization records found under $old_root"
  exit 0
fi

migrated=0

for record in "${records[@]}"; do
  old_dir=$(dirname "$record")
  workdir_id=$(basename "$old_dir")
  new_dir=$workdir_target/$workdir_id
  new_record=$new_dir/materialization.json
  new_checkout=$new_dir/checkout

  old_checkout=$(json_string "$record" '.root')
  source_repository_path=$(json_string "$record" '.source_repository_path')

  [[ "$old_checkout" = "$old_dir"/root/* ]] || fail "$record root is not under expected old root/<repository-id> layout: $old_checkout"
  [[ -d "$old_checkout" ]] || fail "$record root does not exist or is not a directory: $old_checkout"
  [[ -d "$source_repository_path" ]] || fail "$record source_repository_path does not exist or is not a directory: $source_repository_path"

  root_parent=$(dirname "$old_checkout")
  [[ "$root_parent" = "$old_dir/root" ]] || fail "$record root parent is not $old_dir/root: $root_parent"

  shopt -s nullglob dotglob
  root_children=("$old_dir/root"/*)
  shopt -u nullglob dotglob
  [[ ${#root_children[@]} -eq 1 ]] || fail "$old_dir/root must contain exactly one checkout; found ${#root_children[@]}"
  [[ "${root_children[0]}" = "$old_checkout" ]] || fail "$record root does not match the only checkout under $old_dir/root"

  if [[ -e "$new_dir" && "$new_dir" != "$old_dir" ]]; then
    fail "target workdir directory already exists: $new_dir"
  fi
  if [[ -e "$new_checkout" ]]; then
    fail "target checkout already exists: $new_checkout"
  fi
  if [[ -e "$new_record" ]]; then
    fail "target materialization record already exists: $new_record"
  fi

  log "workdir $workdir_id"
  log "  move checkout: $old_checkout -> $new_checkout"
  log "  rewrite record: $record -> $new_record"
  log "  repair git worktree metadata in: $source_repository_path"

  if [[ $apply -eq 1 ]]; then
    mkdir -p "$new_dir"
    mv "$old_checkout" "$new_checkout"

    tmp_record=$(mktemp "$new_dir/materialization.json.tmp.XXXXXX")
    jq --arg root "$new_checkout" '.root = $root' "$record" > "$tmp_record"
    mv "$tmp_record" "$new_record"

    git -C "$source_repository_path" worktree repair "$new_checkout"

    rm "$record"
    rmdir "$old_dir/root"
    rmdir "$old_dir"
  fi
  migrated=$((migrated + 1))
done

if [[ $apply -eq 1 ]]; then
  rmdir "$old_root" 2>/dev/null || true
  log "migrated $migrated workdir(s)"
else
  log "dry-run complete: $migrated workdir(s) would be migrated"
fi
