#!/usr/bin/env bash
set -euo pipefail

# Starts production Server/Runtime binaries against disposable state and ports.
# This script must never read or write the caller's Yoi data/config directories.

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
server_bin=${YOI_SMOKE_SERVER_BIN:-"$repo_root/target/debug/yoi-server"}
runtime_bin=${YOI_SMOKE_RUNTIME_BIN:-"$repo_root/target/debug/yoi-runtime"}
server_port=${YOI_SMOKE_SERVER_PORT:-48787}
runtime_port=${YOI_SMOKE_RUNTIME_PORT:-48800}
keep=${YOI_SMOKE_KEEP:-0}

fail() {
    printf 'isolated-startup-smoke: %s\n' "$*" >&2
    exit 1
}

for command in curl git node ss; do
    command -v "$command" >/dev/null || fail "required command is unavailable: $command"
done
[[ -x "$server_bin" ]] || fail "Server binary is not executable: $server_bin"
[[ -x "$runtime_bin" ]] || fail "Runtime binary is not executable: $runtime_bin"
[[ "$server_port" =~ ^[0-9]+$ ]] || fail "invalid Server port: $server_port"
[[ "$runtime_port" =~ ^[0-9]+$ ]] || fail "invalid Runtime port: $runtime_port"
[[ "$server_port" != "$runtime_port" ]] || fail "Server and Runtime ports must differ"

port_is_listening() {
    local port=$1
    ss -H -ltn "sport = :$port" | grep -q .
}

port_is_listening "$server_port" && fail "Server smoke port is already in use: $server_port"
port_is_listening "$runtime_port" && fail "Runtime smoke port is already in use: $runtime_port"

root=$(mktemp -d "${TMPDIR:-/tmp}/yoi-isolated-startup-smoke.XXXXXX")
server_pid=
runtime_pid=

stop_pid() {
    local pid=${1:-}
    [[ -n "$pid" ]] || return 0
    kill -TERM "$pid" 2>/dev/null || true
    for _ in $(seq 1 50); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.1
    done
    if kill -0 "$pid" 2>/dev/null; then
        kill -KILL "$pid" 2>/dev/null || true
    fi
    wait "$pid" 2>/dev/null || true
}

cleanup() {
    local status=$?
    trap - EXIT INT TERM
    stop_pid "$runtime_pid"
    stop_pid "$server_pid"
    if [[ "$keep" == 1 ]]; then
        printf 'isolated-startup-smoke: kept artifacts at %s\n' "$root" >&2
    elif [[ $status -eq 0 ]]; then
        rm -rf "$root"
    else
        printf 'isolated-startup-smoke: failed; artifacts kept at %s\n' "$root" >&2
    fi
    exit "$status"
}
trap cleanup EXIT INT TERM

mkdir -p "$root/home" "$root/data" "$root/config" "$root/repository" "$root/logs"
export HOME="$root/home"
export XDG_DATA_HOME="$root/data"
export XDG_CONFIG_HOME="$root/config"
unset YOI_DATA_DIR YOI_CONFIG_HOME

# Fail closed if isolation variables no longer point below the disposable root.
case "$HOME:$XDG_DATA_HOME:$XDG_CONFIG_HOME" in
    "$root"/*:"$root"/*:"$root"/*) ;;
    *) fail "HOME/XDG isolation guard failed" ;;
esac

server_url="http://127.0.0.1:$server_port"
runtime_url="http://127.0.0.1:$runtime_port"
server_id=isolated-smoke-server
runtime_id=isolated-smoke-runtime

git -C "$root/repository" init -q
git -C "$root/repository" config user.email smoke@example.invalid
git -C "$root/repository" config user.name 'Yoi isolated smoke'
printf '# isolated smoke\n' >"$root/repository/README.md"
git -C "$root/repository" add README.md
git -C "$root/repository" commit -qm 'test: initialize isolated smoke repository'

"$server_bin" identity init --server-id "$server_id" >"$root/logs/server-identity-init.log" 2>&1
"$runtime_bin" identity init --runtime-id "$runtime_id" >"$root/logs/runtime-identity-init.log" 2>&1
server_identity=$("$server_bin" identity show --json)
runtime_identity=$("$runtime_bin" identity show --json)
server_key=$(printf '%s' "$server_identity" | node -e 'const fs=require("fs"); process.stdout.write(JSON.parse(fs.readFileSync(0,"utf8")).public_key)')
runtime_key=$(printf '%s' "$runtime_identity" | node -e 'const fs=require("fs"); process.stdout.write(JSON.parse(fs.readFileSync(0,"utf8")).public_key)')

"$server_bin" trust-runtime add \
    --runtime-id "$runtime_id" \
    --public-key "$runtime_key" \
    --base-url "$runtime_url" >"$root/logs/server-trust-runtime.log" 2>&1
"$runtime_bin" trust-server add \
    --server-id "$server_id" \
    --public-key "$server_key" >"$root/logs/runtime-trust-server.log" 2>&1

"$server_bin" init --workspace "$root/repository" >"$root/logs/server-init.log" 2>&1
workspace_id=$(sed -n 's/^workspace_id = "\([^"]*\)"/\1/p' "$root/repository/.yoi/workspace.toml")
[[ -n "$workspace_id" ]] || fail "workspace init did not write workspace_id"
# Runtime Git materialization requires a clean source repository. Commit both
# local bootstrap markers inside this disposable repository.
git -C "$root/repository" add .yoi/workspace.toml .yoi/workspace-backend.local.toml
git -C "$root/repository" commit -qm 'test: record isolated Yoi workspace markers'

runtime_store="$XDG_DATA_HOME/yoi/runtime"
mkdir -p "$runtime_store/workers"
cat >"$runtime_store/runtime.json" <<'JSON'
{
  "schema_version": 1,
  "display_name": "isolated startup smoke",
  "backend": "fs_store",
  "status": "running",
  "next_worker_sequence": 1,
  "next_diagnostic_id": 1,
  "config_bundles": {},
  "workspace_owners": {},
  "diagnostics": []
}
JSON

start_server() {
    : >"$root/logs/server.log"
    "$server_bin" serve --listen "127.0.0.1:$server_port" >"$root/logs/server.log" 2>&1 &
    server_pid=$!
}

start_runtime() {
    : >"$root/logs/runtime.log"
    "$runtime_bin" --bind "127.0.0.1:$runtime_port" >"$root/logs/runtime.log" 2>&1 &
    runtime_pid=$!
}

wait_for_listener() {
    local pid=$1
    local port=$2
    local name=$3
    for _ in $(seq 1 150); do
        kill -0 "$pid" 2>/dev/null || fail "$name exited before listening; inspect $root/logs"
        port_is_listening "$port" && return 0
        sleep 0.1
    done
    fail "$name did not listen on port $port within 15 seconds"
}

runtime_projection() {
    curl --fail --silent --show-error \
        "$server_url/api/w/$workspace_id/runtimes"
}

projection_is_ready() {
    node -e '
const fs = require("fs");
const runtimeId = process.argv[1];
const body = JSON.parse(fs.readFileSync(0, "utf8"));
const runtime = body.items.find((item) => item.runtime_id === runtimeId);
if (!runtime || runtime.status !== "running") process.exit(1);
if (!runtime.capabilities?.can_list_workers) process.exit(1);
if ((runtime.diagnostics ?? []).length !== 0) process.exit(1);
if ((body.diagnostics ?? []).length !== 0) process.exit(1);
' "$runtime_id"
}

wait_for_projection_state() {
    local expected=$1
    local body=
    for _ in $(seq 1 150); do
        kill -0 "$server_pid" 2>/dev/null || fail "Server exited during readiness check"
        body=$(runtime_projection 2>/dev/null || true)
        if [[ -n "$body" ]]; then
            if printf '%s' "$body" | projection_is_ready 2>/dev/null; then
                [[ "$expected" == ready ]] && return 0
            else
                [[ "$expected" == not-ready ]] && return 0
            fi
        fi
        sleep 0.1
    done
    printf '%s\n' "$body" >"$root/logs/last-runtime-projection.json"
    fail "Runtime projection did not become $expected within 15 seconds"
}

assert_clean_logs() {
    if grep -Eiq 'panicked at|thread .* panicked|UNIQUE constraint failed|worker_execution_restore_failed' \
        "$root/logs/server.log" "$root/logs/runtime.log"; then
        fail "panic, migration collision, or restore failure found in startup logs"
    fi
}

start_server
wait_for_listener "$server_pid" "$server_port" Server

# Negative control: a listening Server is not readiness. The configured remote
# Runtime must be rejected while it is absent.
wait_for_projection_state not-ready

start_runtime
wait_for_listener "$runtime_pid" "$runtime_port" Runtime
wait_for_projection_state ready
assert_clean_logs

# Listener/catalog readiness is insufficient. Materialize a real Workdir and
# require the normal Server -> Runtime Worker spawn path to create a persisted
# Worker with an execution handle. This catches adapter panics that startup
# alone cannot observe.
repositories=$(curl --fail --silent --show-error \
    "$server_url/api/w/$workspace_id/repositories")
repository_id=$(printf '%s' "$repositories" | node -e '
const fs = require("fs");
const body = JSON.parse(fs.readFileSync(0, "utf8"));
if (body.items.length !== 1) process.exit(1);
process.stdout.write(body.items[0].id);
')
workdir_response=$(curl --fail --silent --show-error \
    --request POST \
    --header 'content-type: application/json' \
    --data "{\"runtime_id\":\"$runtime_id\",\"repository_id\":\"$repository_id\"}" \
    "$server_url/api/w/$workspace_id/runtimes/$runtime_id/working-directories") || \
    fail "isolated Workdir materialization failed"
working_directory_id=$(printf '%s' "$workdir_response" | node -e '
const fs = require("fs");
const body = JSON.parse(fs.readFileSync(0, "utf8"));
if (body.item?.status !== "active" || body.item?.cleanliness !== "clean") process.exit(1);
process.stdout.write(body.item.working_directory_id);
')

cat >"$root/worker-create.json" <<JSON
{
  "runtime_id": "$runtime_id",
  "display_name": "isolated restore smoke",
  "profile": "builtin:companion",
  "initial_submit": [],
  "working_directory": {
    "working_directory_id": "$working_directory_id"
  }
}
JSON
worker_response=$(curl --fail --silent --show-error \
    --request POST \
    --header 'content-type: application/json' \
    --data @"$root/worker-create.json" \
    "$server_url/api/w/$workspace_id/workers") || \
    fail "isolated Worker spawn failed; listener readiness is not sufficient"
worker_id=$(printf '%s' "$worker_response" | node -e '
const fs = require("fs");
const body = JSON.parse(fs.readFileSync(0, "utf8"));
if (body.runtime_id !== process.argv[1] || !body.worker_id) process.exit(1);
process.stdout.write(String(body.worker_id));
' "$runtime_id")
node -e '
const fs = require("fs");
const record = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
const expectedBase = process.argv[2];
const profileUrl = record.request?.profile_source?.location?.url;
const workspaceUrl = record.request?.workspace_api?.base_url;
if (!profileUrl?.startsWith(`${expectedBase}/`)) {
  console.error(`profile callback escaped isolated Server: ${profileUrl}`);
  process.exit(1);
}
if (workspaceUrl !== expectedBase) {
  console.error(`Workspace API escaped isolated Server: ${workspaceUrl}`);
  process.exit(1);
}
' "$runtime_store/workers/$worker_id/worker.json" "$server_url" || \
    fail "persisted Worker callback URLs are not isolated"
assert_clean_logs

# Exercise persistence reopen with a real persisted Worker and require the
# Server projection to recover. The Worker record must remain addressable after
# Runtime restart; restore failures and adapter panics are rejected by log scan.
stop_pid "$runtime_pid"
runtime_pid=
wait_for_projection_state not-ready
start_runtime
wait_for_listener "$runtime_pid" "$runtime_port" Runtime
wait_for_projection_state ready
curl --fail --silent --show-error \
    "$server_url/api/w/$workspace_id/runtimes/$runtime_id/workers/$worker_id" \
    >"$root/logs/restored-worker.json" || fail "persisted Worker is unavailable after Runtime restart"
assert_clean_logs

# Prove that this run used only disposable state paths.
grep -Fq "$root/data/yoi/server/server.db" "$root/logs/server.log" || \
    fail "Server log does not identify the isolated database"
if grep -Fq '/home/hare/.local/share/yoi' "$root/logs/server.log" "$root/logs/runtime.log"; then
    fail "startup logs reference a non-isolated Yoi data path"
fi

printf 'isolated-startup-smoke: PASS (workspace=%s, server=%s, runtime=%s)\n' \
    "$workspace_id" "$server_url" "$runtime_url"
