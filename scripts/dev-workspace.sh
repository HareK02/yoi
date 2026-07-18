#!/usr/bin/env bash
# Development process switcher for the workspace web/backend/runtime stack.
#
# Common usage:
#   scripts/dev-workspace.sh status    # inspect managed pids and port listeners only
#   scripts/dev-workspace.sh restart   # restart backend/runtime only; frontend is left running
#   scripts/dev-workspace.sh start     # move runtime/backend/frontend listeners to this checkout
#   scripts/dev-workspace.sh stop      # stop runtime/backend/frontend listeners
#
# Safety notes:
# - start/stop/restart are detached by default and run after
#   YOI_DEV_ACTION_DELAY_SECONDS=60. This gives API/tool-call sessions time to
#   persist their result before backend/runtime processes are stopped.
# - Use restart for normal backend/runtime code changes. It intentionally does
#   not touch the frontend dev server.
# - Use start or stop when the frontend listener must also move between
#   worktrees; frontend binds to 0.0.0.0 by default for browser access.
# - Avoid YOI_DEV_WORKSPACE_FOREGROUND=1 during active API sessions; it runs the
#   mutating action synchronously and can interrupt the session that invoked it.
# - Check the printed scheduled_log after a detached action completes.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNTIME_DIR="${YOI_DEV_RUNTIME_DIR:-$ROOT_DIR/.yoi/dev}"
PID_DIR="$RUNTIME_DIR/pids"
LOG_DIR="$RUNTIME_DIR/logs"

BACKEND_LISTEN="${YOI_DEV_BACKEND_LISTEN:-127.0.0.1:8787}"
RUNTIME_BIND="${YOI_DEV_RUNTIME_BIND:-127.0.0.1:38800}"
FRONTEND_HOST="${YOI_DEV_FRONTEND_HOST:-0.0.0.0}"
FRONTEND_PORT="${YOI_DEV_FRONTEND_PORT:-5173}"

RUNTIME_ENABLED="${YOI_DEV_RUNTIME_ENABLED:-1}"
ACTION_DELAY_SECONDS="${YOI_DEV_ACTION_DELAY_SECONDS:-60}"
FOREGROUND_MODE="${YOI_DEV_WORKSPACE_FOREGROUND:-0}"

usage() {
  cat <<EOF
Usage: $(basename "$0") <start|stop|restart|status>

Manage the local Yoi development stack for this checkout:
  runtime   cargo run -p worker-runtime --features ws-server,fs-store --bin worker-runtime-rest-server -- --bind $RUNTIME_BIND
  backend   cargo run -p yoi-workspace-server -- serve --workspace $ROOT_DIR --db $ROOT_DIR/.yoi/workspace.db --listen $BACKEND_LISTEN
  frontend  deno run -A npm:vite@7.2.7 dev --host $FRONTEND_HOST --port $FRONTEND_PORT  (cwd: web/workspace)

Actions:
  start     schedule a detached job that stops existing listeners, then starts runtime, backend, and frontend from this checkout
  stop      schedule a detached job that stops runtime, backend, and frontend
  restart   schedule a detached job that stops/starts runtime and backend only; frontend is left untouched
  status    print pidfile and port-listener status without mutating processes

By default, start/stop/restart return immediately and run in a detached job after $ACTION_DELAY_SECONDS seconds.
This keeps API/tool-call sessions intact while the backend/runtime are restarted. Set
YOI_DEV_WORKSPACE_FOREGROUND=1 to run the mutating action synchronously.

Environment overrides:
  YOI_DEV_ACTION_DELAY_SECONDS=60  delay before detached mutating actions run
  YOI_DEV_WORKSPACE_FOREGROUND=1   run start/stop/restart synchronously instead of scheduling
  YOI_DEV_BACKEND_LISTEN=127.0.0.1:8787
  YOI_DEV_RUNTIME_BIND=127.0.0.1:38800
  YOI_DEV_RUNTIME_ENABLED=1        set to 0 to skip the standalone runtime process
  YOI_DEV_FRONTEND_HOST=0.0.0.0
  YOI_DEV_FRONTEND_PORT=5173
  YOI_DEV_RUNTIME_DIR=$ROOT_DIR/.yoi/dev
EOF
}

log() {
  printf '[dev-workspace] %s\n' "$*" >&2
}

ensure_dirs() {
  mkdir -p "$PID_DIR" "$LOG_DIR"
}

pid_file() {
  printf '%s/%s.pid' "$PID_DIR" "$1"
}

log_file() {
  printf '%s/%s.log' "$LOG_DIR" "$1"
}

is_running() {
  local pid="$1"
  [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null
}

service_pid() {
  local file
  file="$(pid_file "$1")"
  [[ -f "$file" ]] || return 1
  local pid
  pid="$(cat "$file")"
  is_running "$pid" || return 1
  printf '%s' "$pid"
}

split_addr_port() {
  local value="$1"
  local port="${value##*:}"
  local host="${value%:*}"
  if [[ "$host" == "$value" || -z "$port" ]]; then
    printf 'invalid address:port value: %s\n' "$value" >&2
    return 1
  fi
  printf '%s\t%s\n' "$host" "$port"
}

port_for_addr() {
  split_addr_port "$1" | awk '{ print $2 }'
}

listener_pids_for_port() {
  local port="$1"
  if command -v lsof >/dev/null 2>&1; then
    lsof -nP -iTCP:"$port" -sTCP:LISTEN -t 2>/dev/null | sort -u
    return 0
  fi
  if command -v ss >/dev/null 2>&1; then
    ss -ltnp "sport = :$port" 2>/dev/null \
      | sed -nE 's/.*pid=([0-9]+).*/\1/p' \
      | sort -u
    return 0
  fi
}

stop_pid() {
  local pid="$1"
  local label="$2"
  is_running "$pid" || return 0

  local pgid
  pgid="$(ps -o pgid= -p "$pid" 2>/dev/null | tr -d '[:space:]' || true)"
  if [[ "$pgid" == "$pid" ]]; then
    log "stopping $label process group -$pid"
    kill -TERM "-$pid" 2>/dev/null || true
  else
    log "stopping $label pid $pid"
    kill -TERM "$pid" 2>/dev/null || true
  fi

  for _ in {1..50}; do
    is_running "$pid" || return 0
    sleep 0.1
  done

  log "forcing $label pid $pid"
  if [[ "$pgid" == "$pid" ]]; then
    kill -KILL "-$pid" 2>/dev/null || true
  else
    kill -KILL "$pid" 2>/dev/null || true
  fi
}

stop_managed_service() {
  local name="$1"
  local file
  file="$(pid_file "$name")"
  if [[ ! -f "$file" ]]; then
    return 0
  fi

  local pid
  pid="$(cat "$file")"
  if is_running "$pid"; then
    stop_pid "$pid" "$name"
  fi
  rm -f "$file"
}

stop_port_listeners() {
  local label="$1"
  local port="$2"
  local pids=()
  mapfile -t pids < <(listener_pids_for_port "$port" || true)
  if [[ "${#pids[@]}" -eq 0 ]]; then
    return 0
  fi

  for pid in "${pids[@]}"; do
    [[ -n "$pid" ]] || continue
    log "stopping existing $label listener on port $port pid $pid"
    # Unmanaged dev processes may share a process group with the caller's terminal
    # or pod; stop only the listener PID here. Managed processes started by this
    # script are stopped by process group via pidfiles above.
    kill -TERM "$pid" 2>/dev/null || true
  done

  for _ in {1..50}; do
    mapfile -t pids < <(listener_pids_for_port "$port" || true)
    [[ "${#pids[@]}" -eq 0 ]] && return 0
    sleep 0.1
  done

  for pid in "${pids[@]}"; do
    [[ -n "$pid" ]] || continue
    log "forcing existing $label listener on port $port pid $pid"
    kill -KILL "$pid" 2>/dev/null || true
  done
}

start_service() {
  local name="$1"
  local cwd="$2"
  local port="$3"
  shift 3

  stop_managed_service "$name"
  stop_port_listeners "$name" "$port"

  local logfile pidfile
  logfile="$(log_file "$name")"
  pidfile="$(pid_file "$name")"
  log "starting $name; log: $logfile"
  (
    cd "$cwd"
    exec setsid "$@"
  ) >"$logfile" 2>&1 &
  local pid="$!"
  printf '%s\n' "$pid" >"$pidfile"
  log "$name pid $pid"
}

start_runtime() {
  if [[ "$RUNTIME_ENABLED" == "0" ]]; then
    log "runtime disabled by YOI_DEV_RUNTIME_ENABLED=0"
    return 0
  fi
  local port
  port="$(port_for_addr "$RUNTIME_BIND")"
  start_service runtime "$ROOT_DIR" "$port" \
    cargo run -p worker-runtime --features ws-server,fs-store --bin worker-runtime-rest-server -- --bind "$RUNTIME_BIND"
}

start_backend() {
  local port
  port="$(port_for_addr "$BACKEND_LISTEN")"
  start_service backend "$ROOT_DIR" "$port" \
    cargo run -p yoi-workspace-server -- serve --workspace "$ROOT_DIR" --db "$ROOT_DIR/.yoi/workspace.db" --listen "$BACKEND_LISTEN"
}

start_frontend() {
  local frontend_dir="$ROOT_DIR/web/workspace"
  start_service frontend "$frontend_dir" "$FRONTEND_PORT" \
    deno run -A npm:vite@7.2.7 dev --host "$FRONTEND_HOST" --port "$FRONTEND_PORT"
}

stop_runtime_backend() {
  stop_managed_service backend
  stop_managed_service runtime
  stop_port_listeners backend "$(port_for_addr "$BACKEND_LISTEN")"
  if [[ "$RUNTIME_ENABLED" != "0" ]]; then
    stop_port_listeners runtime "$(port_for_addr "$RUNTIME_BIND")"
  fi
}

start_runtime_backend() {
  start_runtime
  start_backend
}

start_all() {
  ensure_dirs
  start_runtime_backend
  start_frontend
}

stop_all() {
  ensure_dirs
  stop_managed_service frontend
  stop_runtime_backend
  stop_port_listeners frontend "$FRONTEND_PORT"
}

restart_runtime_backend() {
  ensure_dirs
  log "restarting runtime/backend only; frontend is left untouched"
  stop_runtime_backend
  start_runtime_backend
}

status_service() {
  local name="$1"
  local port="$2"
  local managed="-"
  local listeners="-"
  if service_pid "$name" >/dev/null; then
    managed="$(service_pid "$name")"
  fi
  mapfile -t pids < <(listener_pids_for_port "$port" || true)
  if [[ "${#pids[@]}" -gt 0 ]]; then
    listeners="${pids[*]}"
  fi
  printf '%-8s managed_pid=%-8s port=%-6s listener_pids=%s\n' "$name" "$managed" "$port" "$listeners"
}

status_all() {
  ensure_dirs
  status_service runtime "$(port_for_addr "$RUNTIME_BIND")"
  status_service backend "$(port_for_addr "$BACKEND_LISTEN")"
  status_service frontend "$FRONTEND_PORT"
}

schedule_detached_action() {
  local action="$1"
  ensure_dirs

  local stamp job_log
  stamp="$(date +%Y%m%d-%H%M%S)"
  job_log="$LOG_DIR/${action}-$stamp.job.log"

  log "scheduling $action in ${ACTION_DELAY_SECONDS}s; log: $job_log"
  setsid bash -lc '
    set -euo pipefail
    delay="$1"
    root="$2"
    action="$3"
    sleep "$delay"
    cd "$root"
    printf "[%s] dev-workspace %s starting\n" "$(date -Is)" "$action"
    YOI_DEV_WORKSPACE_FOREGROUND=1 "$root/scripts/dev-workspace.sh" "$action"
    status=$?
    printf "[%s] dev-workspace %s finished with status %s\n" "$(date -Is)" "$action" "$status"
    exit "$status"
  ' dev-workspace-job "$ACTION_DELAY_SECONDS" "$ROOT_DIR" "$action" >>"$job_log" 2>&1 < /dev/null &

  printf 'scheduled_action=%s\n' "$action"
  printf 'scheduled_pid=%s\n' "$!"
  printf 'scheduled_after_seconds=%s\n' "$ACTION_DELAY_SECONDS"
  printf 'scheduled_log=%s\n' "$job_log"
}

run_mutating_action() {
  local action="$1"
  if [[ "$FOREGROUND_MODE" != "1" ]]; then
    schedule_detached_action "$action"
    return 0
  fi

  case "$action" in
    start)
      start_all
      ;;
    stop)
      stop_all
      ;;
    restart)
      restart_runtime_backend
      ;;
    *)
      printf 'unknown mutating action: %s\n' "$action" >&2
      return 2
      ;;
  esac
}

main() {
  local action="${1:-}"
  case "$action" in
    start)
      run_mutating_action start
      ;;
    stop)
      run_mutating_action stop
      ;;
    restart)
      run_mutating_action restart
      ;;
    status)
      status_all
      ;;
    -h|--help|help)
      usage
      ;;
    "")
      usage
      exit 2
      ;;
    *)
      printf 'unknown action: %s\n\n' "$action" >&2
      usage >&2
      exit 2
      ;;
  esac
}

main "$@"
