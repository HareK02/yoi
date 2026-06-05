#!/bin/sh
set -eu

WORK_ITEMS_DIR=${WORK_ITEMS_DIR:-.yoi/tickets}
STATUSES="open pending closed"
REQUIRED_FIELDS="id slug title status kind priority labels created_at updated_at assignee legacy_ticket"

usage() {
    cat <<'EOF'
tickets.sh - transitional repository-local Ticket helper

Usage:
  ./tickets.sh help
  ./tickets.sh --help
  ./tickets.sh list [--status open|pending|closed|all]
  ./tickets.sh show <id-or-slug>
  ./tickets.sh create --title <title> [--slug <slug>] [--kind <kind>] [--priority P2] [--label a,b]
  ./tickets.sh comment <id-or-slug> [--role comment|plan|decision|implementation_report] [--author <name>] [--file <path>]
  ./tickets.sh review <id-or-slug> --approve|--request-changes [--author <name>] [--file <path>]
  ./tickets.sh status <id-or-slug> open|pending|closed
  ./tickets.sh close <id-or-slug> [--resolution <text>|--file <path>]
  ./tickets.sh doctor

Backend:
  .yoi/tickets/{open,pending,closed}/<id>/item.md
  .yoi/tickets/{open,pending,closed}/<id>/thread.md
  .yoi/tickets/{open,pending,closed}/<id>/artifacts/

Transition policy:
  .yoi/tickets/ is the active built-in local Ticket backend. This script remains
  a maintainer shim until yoi ticket fully replaces it; WORK_ITEMS_DIR may be
  set for one-off legacy/recovery checks, but do not maintain a second live root.
  Review notes must be appended to thread.md instead of tickets/*.review.md.
EOF
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

now_utc() {
    date -u '+%Y-%m-%dT%H:%M:%SZ'
}

compact_date() {
    date -u '+%Y%m%d-%H%M%S'
}

ensure_backend_dirs() {
    mkdir -p "$WORK_ITEMS_DIR/open" "$WORK_ITEMS_DIR/pending" "$WORK_ITEMS_DIR/closed"
}

is_status() {
    case "$1" in
        open|pending|closed) return 0 ;;
        *) return 1 ;;
    esac
}

slugify() {
    # ASCII-focused slugifier for POSIX shell. Non-ASCII titles should pass --slug.
    printf '%s' "$1" |
        tr '[:upper:]' '[:lower:]' |
        sed 's/[^a-z0-9][^a-z0-9]*/-/g; s/^-//; s/-$//; s/--*/-/g'
}

field_value() {
    file=$1
    field=$2
    awk -v key="$field" '
        NR == 1 && $0 == "---" { in_fm = 1; next }
        in_fm && $0 == "---" { exit }
        in_fm {
            prefix = key ":"
            if (index($0, prefix) == 1) {
                value = substr($0, length(prefix) + 1)
                sub(/^[[:space:]]*/, "", value)
                print value
                exit
            }
        }
    ' "$file"
}

set_frontmatter_field() {
    file=$1
    field=$2
    value=$3
    tmp=${TMPDIR:-/tmp}/tickets-sh.$$.tmp
    awk -v key="$field" -v new_value="$value" '
        NR == 1 && $0 == "---" { in_fm = 1; print; next }
        in_fm && $0 == "---" { in_fm = 0; print; next }
        in_fm {
            prefix = key ":"
            if (index($0, prefix) == 1) {
                print key ": " new_value
                next
            }
        }
        { print }
    ' "$file" > "$tmp"
    mv "$tmp" "$file"
}

find_item_dir() {
    query=$1
    matches=${TMPDIR:-/tmp}/tickets-sh.matches.$$
    : > "$matches"
    for status in $STATUSES; do
        for dir in "$WORK_ITEMS_DIR/$status"/*; do
            [ -d "$dir" ] || continue
            item=$dir/item.md
            [ -f "$item" ] || continue
            id=$(field_value "$item" id || true)
            slug=$(field_value "$item" slug || true)
            if [ "$query" = "$id" ] || [ "$query" = "$slug" ]; then
                printf '%s\n' "$dir" >> "$matches"
            fi
        done
    done
    count=$(wc -l < "$matches" | tr -d ' ')
    if [ "$count" -eq 0 ]; then
        rm -f "$matches"
        die "work item not found: $query"
    fi
    if [ "$count" -gt 1 ]; then
        cat "$matches" >&2
        rm -f "$matches"
        die "ambiguous work item: $query"
    fi
    sed -n '1p' "$matches"
    rm -f "$matches"
}

item_status_from_dir() {
    case "$1" in
        */open/*) printf 'open\n' ;;
        */pending/*) printf 'pending\n' ;;
        */closed/*) printf 'closed\n' ;;
        *) printf 'unknown\n' ;;
    esac
}

labels_yaml() {
    labels=$1
    if [ -z "$labels" ]; then
        printf '[]'
        return
    fi
    old_ifs=$IFS
    IFS=,
    first=1
    printf '['
    for label in $labels; do
        clean=$(printf '%s' "$label" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//')
        [ -n "$clean" ] || continue
        if [ "$first" -eq 0 ]; then
            printf ', '
        fi
        printf '%s' "$clean"
        first=0
    done
    IFS=$old_ifs
    printf ']'
}

read_body_to_file() {
    input_file=$1
    output_file=$2
    if [ -n "$input_file" ]; then
        [ -f "$input_file" ] || die "file not found: $input_file"
        cat "$input_file" > "$output_file"
        return
    fi
    if [ -t 0 ]; then
        printf 'No body provided.\n' > "$output_file"
    else
        cat > "$output_file"
    fi
}

append_thread_event() {
    dir=$1
    event=$2
    heading=$3
    author=$4
    status_value=$5
    body_file=$6
    at=$(now_utc)
    thread=$dir/thread.md
    [ -f "$thread" ] || : > "$thread"
    {
        printf '\n<!-- event: %s author: %s at: %s' "$event" "$author" "$at"
        if [ -n "$status_value" ]; then
            printf ' status: %s' "$status_value"
        fi
        printf ' -->\n\n'
        printf '## %s\n\n' "$heading"
        cat "$body_file"
        printf '\n\n---\n'
    } >> "$thread"
    set_frontmatter_field "$dir/item.md" updated_at "$at"
}

cmd_list() {
    status_filter=open
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --status)
                [ "$#" -ge 2 ] || die "--status requires a value"
                status_filter=$2
                shift 2
                ;;
            *) die "unknown list argument: $1" ;;
        esac
    done
    case "$status_filter" in
        open|pending|closed|all) ;;
        *) die "invalid status: $status_filter" ;;
    esac

    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' status id slug title kind priority updated_at
    for status in $STATUSES; do
        if [ "$status_filter" != all ] && [ "$status_filter" != "$status" ]; then
            continue
        fi
        for dir in "$WORK_ITEMS_DIR/$status"/*; do
            [ -d "$dir" ] || continue
            item=$dir/item.md
            [ -f "$item" ] || continue
            printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
                "$(field_value "$item" status)" \
                "$(field_value "$item" id)" \
                "$(field_value "$item" slug)" \
                "$(field_value "$item" title)" \
                "$(field_value "$item" kind)" \
                "$(field_value "$item" priority)" \
                "$(field_value "$item" updated_at)"
        done
    done
}

cmd_show() {
    [ "$#" -eq 1 ] || die "show requires <id-or-slug>"
    dir=$(find_item_dir "$1")
    item=$dir/item.md
    thread=$dir/thread.md
    printf '# %s\n\n' "$(field_value "$item" title)"
    printf 'Path: %s\n' "$dir"
    printf 'Status: %s\n' "$(field_value "$item" status)"
    printf 'ID: %s\n' "$(field_value "$item" id)"
    printf 'Slug: %s\n\n' "$(field_value "$item" slug)"
    printf '## item.md\n\n'
    cat "$item"
    printf '\n\n## thread.md\n\n'
    if [ -f "$thread" ]; then
        tail -n 80 "$thread"
    else
        printf '(missing thread.md)\n'
    fi
}

cmd_create() {
    title=
    slug=
    kind=task
    priority=P2
    labels=
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --title) [ "$#" -ge 2 ] || die "--title requires a value"; title=$2; shift 2 ;;
            --slug) [ "$#" -ge 2 ] || die "--slug requires a value"; slug=$2; shift 2 ;;
            --kind) [ "$#" -ge 2 ] || die "--kind requires a value"; kind=$2; shift 2 ;;
            --priority) [ "$#" -ge 2 ] || die "--priority requires a value"; priority=$2; shift 2 ;;
            --label) [ "$#" -ge 2 ] || die "--label requires a value"; labels=$2; shift 2 ;;
            *) die "unknown create argument: $1" ;;
        esac
    done
    [ -n "$title" ] || die "create requires --title"
    if [ -z "$slug" ]; then
        slug=$(slugify "$title")
    else
        slug=$(slugify "$slug")
    fi
    [ -n "$slug" ] || slug=item
    ensure_backend_dirs
    stamp=$(compact_date)
    id=$stamp-$slug
    dir=$WORK_ITEMS_DIR/open/$id
    if [ -e "$dir" ]; then
        id=$id-$$
        dir=$WORK_ITEMS_DIR/open/$id
    fi
    created=$(now_utc)
    mkdir -p "$dir/artifacts"
    : > "$dir/artifacts/.gitkeep"
    cat > "$dir/item.md" <<EOF
---
id: $id
slug: $slug
title: $title
status: open
kind: $kind
priority: $priority
labels: $(labels_yaml "$labels")
created_at: $created
updated_at: $created
assignee: null
legacy_ticket: null
---

## Background

Created by tickets.sh.

## Acceptance criteria

- TBD
EOF
    cat > "$dir/thread.md" <<EOF
<!-- event: create author: tickets.sh at: $created -->

## Created

Created by tickets.sh create.

---
EOF
    printf '%s\n' "$id"
}

cmd_comment() {
    [ "$#" -ge 1 ] || die "comment requires <id-or-slug>"
    query=$1
    shift
    role=comment
    author=${USER:-unknown}
    file=
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --role) [ "$#" -ge 2 ] || die "--role requires a value"; role=$2; shift 2 ;;
            --author) [ "$#" -ge 2 ] || die "--author requires a value"; author=$2; shift 2 ;;
            --file) [ "$#" -ge 2 ] || die "--file requires a value"; file=$2; shift 2 ;;
            *) die "unknown comment argument: $1" ;;
        esac
    done
    dir=$(find_item_dir "$query")
    body=${TMPDIR:-/tmp}/tickets-sh.body.$$
    read_body_to_file "$file" "$body"
    heading=$role
    case "$role" in
        comment) heading="Comment" ;;
        plan) heading="Plan" ;;
        decision) heading="Decision" ;;
        implementation_report) heading="Implementation report" ;;
    esac
    append_thread_event "$dir" "$role" "$heading" "$author" "" "$body"
    rm -f "$body"
}

cmd_review() {
    [ "$#" -ge 1 ] || die "review requires <id-or-slug>"
    query=$1
    shift
    author=${USER:-unknown}
    file=
    review_status=
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --approve) review_status=approve; shift ;;
            --request-changes) review_status=request_changes; shift ;;
            --author) [ "$#" -ge 2 ] || die "--author requires a value"; author=$2; shift 2 ;;
            --file) [ "$#" -ge 2 ] || die "--file requires a value"; file=$2; shift 2 ;;
            *) die "unknown review argument: $1" ;;
        esac
    done
    [ -n "$review_status" ] || die "review requires --approve or --request-changes"
    dir=$(find_item_dir "$query")
    body=${TMPDIR:-/tmp}/tickets-sh.body.$$
    read_body_to_file "$file" "$body"
    if [ "$review_status" = approve ]; then
        heading="Review: approve"
    else
        heading="Review: request changes"
    fi
    append_thread_event "$dir" review "$heading" "$author" "$review_status" "$body"
    rm -f "$body"
}

cmd_status() {
    [ "$#" -eq 2 ] || die "status requires <id-or-slug> <open|pending|closed>"
    query=$1
    new_status=$2
    is_status "$new_status" || die "invalid status: $new_status"
    dir=$(find_item_dir "$query")
    id=$(field_value "$dir/item.md" id)
    new_dir=$WORK_ITEMS_DIR/$new_status/$id
    ensure_backend_dirs
    if [ "$dir" != "$new_dir" ]; then
        [ ! -e "$new_dir" ] || die "target already exists: $new_dir"
        mv "$dir" "$new_dir"
        dir=$new_dir
    fi
    set_frontmatter_field "$dir/item.md" status "$new_status"
    set_frontmatter_field "$dir/item.md" updated_at "$(now_utc)"
}

cmd_close() {
    [ "$#" -ge 1 ] || die "close requires <id-or-slug>"
    query=$1
    shift
    resolution=
    close_file=
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --resolution) [ "$#" -ge 2 ] || die "--resolution requires a value"; resolution=$2; shift 2 ;;
            --file) [ "$#" -ge 2 ] || die "--file requires a value"; close_file=$2; shift 2 ;;
            *) die "unknown close argument: $1" ;;
        esac
    done
    cmd_status "$query" closed
    dir=$(find_item_dir "$query")
    body=${TMPDIR:-/tmp}/tickets-sh.body.$$
    if [ -n "$close_file" ]; then
        read_body_to_file "$close_file" "$body"
    elif [ -n "$resolution" ]; then
        printf '%s\n' "$resolution" > "$body"
    else
        printf 'Closed.\n' > "$body"
    fi
    cp "$body" "$dir/resolution.md"
    append_thread_event "$dir" close "Closed" "${USER:-unknown}" closed "$body"
    rm -f "$body"
}

doctor_error() {
    printf 'doctor: %s\n' "$*" >&2
    DOCTOR_ERRORS=$((DOCTOR_ERRORS + 1))
}

cmd_doctor() {
    DOCTOR_ERRORS=0
    for status in $STATUSES; do
        [ -d "$WORK_ITEMS_DIR/$status" ] || doctor_error "missing directory: $WORK_ITEMS_DIR/$status"
    done

    ids=${TMPDIR:-/tmp}/tickets-sh.ids.$$
    slugs=${TMPDIR:-/tmp}/tickets-sh.slugs.$$
    : > "$ids"
    : > "$slugs"

    for status in $STATUSES; do
        [ -d "$WORK_ITEMS_DIR/$status" ] || continue
        for dir in "$WORK_ITEMS_DIR/$status"/*; do
            [ -d "$dir" ] || continue
            item=$dir/item.md
            thread=$dir/thread.md
            artifacts=$dir/artifacts
            [ -f "$item" ] || { doctor_error "missing item.md: $dir"; continue; }
            [ -f "$thread" ] || doctor_error "missing thread.md: $dir"
            [ -d "$artifacts" ] || doctor_error "missing artifacts/: $dir"
            first=$(sed -n '1p' "$item")
            [ "$first" = "---" ] || doctor_error "item.md missing frontmatter opener: $item"
            for field in $REQUIRED_FIELDS; do
                value=$(field_value "$item" "$field" || true)
                [ -n "$value" ] || doctor_error "missing required field '$field': $item"
            done
            id=$(field_value "$item" id || true)
            slug=$(field_value "$item" slug || true)
            fm_status=$(field_value "$item" status || true)
            if [ -n "$id" ]; then
                printf '%s\t%s\n' "$id" "$item" >> "$ids"
                base=$(basename "$dir")
                [ "$base" = "$id" ] || doctor_error "directory id mismatch: $dir has id $id"
            fi
            if [ -n "$slug" ]; then
                printf '%s\t%s\n' "$slug" "$item" >> "$slugs"
            fi
            if [ "$fm_status" != "$status" ]; then
                doctor_error "status mismatch: $item has '$fm_status' under '$status'"
            fi
        done
    done

    dup_ids=$(cut -f1 "$ids" | sort | uniq -d)
    if [ -n "$dup_ids" ]; then
        old_ifs=$IFS
        IFS='
'
        for dup in $dup_ids; do
            [ -n "$dup" ] && doctor_error "duplicate id: $dup"
        done
        IFS=$old_ifs
    fi
    dup_slugs=$(cut -f1 "$slugs" | sort | uniq -d)
    if [ -n "$dup_slugs" ]; then
        old_ifs=$IFS
        IFS='
'
        for dup in $dup_slugs; do
            [ -n "$dup" ] && doctor_error "duplicate slug: $dup"
        done
        IFS=$old_ifs
    fi
    rm -f "$ids" "$slugs"

    if [ -f TODO.md ] && grep -Eq 'tickets/[^][ )]+\.md|tickets/.*\.review\.md' TODO.md; then
        doctor_error "TODO.md still references legacy tickets/*.md"
    fi
    for f in tickets/*.md tickets/*.review.md; do
        [ -e "$f" ] || continue
        doctor_error "legacy ticket file remains: $f"
    done

    if [ "$DOCTOR_ERRORS" -eq 0 ]; then
        printf 'doctor: ok\n'
        return 0
    fi
    printf 'doctor: %s error(s)\n' "$DOCTOR_ERRORS" >&2
    return 1
}

main() {
    cmd=${1:-help}
    case "$cmd" in
        help|--help|-h) usage ;;
        list) shift; cmd_list "$@" ;;
        show) shift; cmd_show "$@" ;;
        create) shift; cmd_create "$@" ;;
        comment) shift; cmd_comment "$@" ;;
        review) shift; cmd_review "$@" ;;
        status) shift; cmd_status "$@" ;;
        close) shift; cmd_close "$@" ;;
        doctor) shift; [ "$#" -eq 0 ] || die "doctor takes no arguments"; cmd_doctor ;;
        *) usage >&2; die "unknown command: $cmd" ;;
    esac
}

main "$@"
