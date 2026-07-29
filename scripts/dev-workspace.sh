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

default_dev_runtime_dir() {
  if [[ -n "${YOI_DEV_RUNTIME_DIR:-}" ]]; then
    printf '%s\n' "$YOI_DEV_RUNTIME_DIR"
  elif [[ -n "${YOI_RUNTIME_DIR:-}" ]]; then
    printf '%s/yoi-dev\n' "$YOI_RUNTIME_DIR"
  elif [[ -n "${YOI_HOME:-}" ]]; then
    printf '%s/run/yoi-dev\n' "$YOI_HOME"
  elif [[ -n "${XDG_RUNTIME_DIR:-}" ]]; then
    printf '%s/yoi/dev\n' "$XDG_RUNTIME_DIR"
  else
    printf '%s/.local/share/yoi/run/dev\n' "${HOME:?HOME is required when XDG_RUNTIME_DIR is unset}"
  fi
}

RUNTIME_DIR="$(default_dev_runtime_dir)"
PID_DIR="$RUNTIME_DIR/pids"
LOG_DIR="$RUNTIME_DIR/logs"

BACKEND_LISTEN="${YOI_DEV_BACKEND_LISTEN:-127.0.0.1:8787}"
RUNTIME_BIND="${YOI_DEV_RUNTIME_BIND:-127.0.0.1:38800}"
FRONTEND_HOST="${YOI_DEV_FRONTEND_HOST:-0.0.0.0}"
FRONTEND_PORT="${YOI_DEV_FRONTEND_PORT:-5173}"

RUNTIME_ENABLED="${YOI_DEV_RUNTIME_ENABLED:-1}"
ACTION_DELAY_SECONDS="${YOI_DEV_ACTION_DELAY_SECONDS:-60}"
WORKDIR_ID="$(basename "$(dirname "$(dirname "$ROOT_DIR")")")"
UNIT_PREFIX="${YOI_DEV_SYSTEMD_UNIT_PREFIX:-yoi-dev-$WORKDIR_ID}"
USE_SYSTEMD="${YOI_DEV_USE_SYSTEMD:-1}"
FOREGROUND_MODE="${YOI_DEV_WORKSPACE_FOREGROUND:-0}"

usage() {
  cat <<EOF
Usage: $(basename "$0") <start|stop|restart|status>

Manage the local Yoi development stack for this checkout:
  runtime   target/debug/yoi-runtime --bind $RUNTIME_BIND
  backend   target/debug/yoi-server serve --listen $BACKEND_LISTEN
  frontend  deno run -A npm:vite@7.2.7 dev --host $FRONTEND_HOST --port $FRONTEND_PORT  (cwd: web/workspace)

Actions:
  start     schedule a detached job that stops existing listeners, then starts runtime, backend, and frontend from this checkout
  stop      schedule a detached job that stops runtime, backend, and frontend
  restart   schedule a detached job that stops/starts runtime and backend only; frontend is left untouched
  status    print pidfile and port-listener status without mutating processes

By default, start/stop/restart return immediately and run in a fully detached nohup+setsid job after $ACTION_DELAY_SECONDS seconds.
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
  YOI_DEV_RUNTIME_DIR=$RUNTIME_DIR  pid/log directory override (defaults to XDG runtime/data fallback)
EOF
}

log() {
  printf '[dev-workspace] %s\n' "$*" >&2
}

run_cargo() {
  if command -v cc >/dev/null 2>&1 && command -v pkg-config >/dev/null 2>&1; then
    cargo "$@"
    return
  fi
  if command -v nix >/dev/null 2>&1 && [[ -f "$ROOT_DIR/flake.nix" ]]; then
    nix develop "$ROOT_DIR" -c cargo "$@"
    return
  fi
  cargo "$@"
}

ensure_dirs() {
  mkdir -p "$PID_DIR" "$LOG_DIR"
}

pid_file() {
  printf '%s/%s.pid' "$PID_DIR" "$1"
}

systemd_unit_name() {
  local name="$1"
  printf '%s-%s.service' "$UNIT_PREFIX" "$name"
}

systemd_available() {
  [[ "$USE_SYSTEMD" != "0" ]] || return 1
  command -v systemd-run >/dev/null 2>&1 || return 1
  systemctl --user is-system-running >/dev/null 2>&1 || return 1
}

systemd_main_pid() {
  local name="$1"
  local unit
  unit="$(systemd_unit_name "$name")"
  systemctl --user show -P MainPID "$unit" 2>/dev/null | awk '$1 != "" && $1 != "0" { print $1; exit }'
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
  local file unit pid
  file="$(pid_file "$name")"

  if systemd_available; then
    unit="$(systemd_unit_name "$name")"
    if systemctl --user is-active --quiet "$unit" 2>/dev/null; then
      log "stopping $name systemd unit $unit"
      systemctl --user stop "$unit" 2>/dev/null || true
    fi
  fi

  if [[ -f "$file" ]]; then
    pid="$(cat "$file")"
    if is_running "$pid"; then
      stop_pid "$pid" "$name"
    fi
    rm -f "$file"
  fi
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

  local logfile pidfile pid
  logfile="$(log_file "$name")"
  pidfile="$(pid_file "$name")"
  : >"$logfile"

  if systemd_available; then
    local unit
    unit="$(systemd_unit_name "$name")"
    log "starting $name systemd unit $unit; log: $logfile"
    systemd-run --user --unit="$unit" --collect --same-dir --working-directory="$cwd" \
      --property="StandardOutput=append:$logfile" \
      --property="StandardError=append:$logfile" \
      --property="KillMode=control-group" \
      "$@" >/dev/null
    for _ in {1..50}; do
      pid="$(systemd_main_pid "$name" || true)"
      if [[ -n "$pid" ]]; then
        printf '%s\n' "$pid" >"$pidfile"
        log "$name systemd pid $pid"
        return 0
      fi
      sleep 0.1
    done
    printf '%s\n' "$name systemd unit did not expose MainPID" >&2
    return 1
  fi

  log "starting $name; log: $logfile"
  (
    cd "$cwd"
    exec setsid "$@"
  ) >"$logfile" 2>&1 &
  pid="$!"
  printf '%s\n' "$pid" >"$pidfile"
  log "$name pid $pid"
}


build_runtime_backend() {
  if [[ "$RUNTIME_ENABLED" != "0" ]]; then
    log "building runtime binary"
    (
      cd "$ROOT_DIR"
      run_cargo build -p worker-runtime --bin yoi-runtime
    )
  else
    log "runtime disabled by YOI_DEV_RUNTIME_ENABLED=0; skipping runtime build"
  fi

  log "building backend binary"
  (
    cd "$ROOT_DIR"
    run_cargo build -p yoi-workspace-server --bin yoi-server
  )
}

start_runtime() {
  if [[ "$RUNTIME_ENABLED" == "0" ]]; then
    log "runtime disabled by YOI_DEV_RUNTIME_ENABLED=0"
    return 0
  fi
  local port runtime_bin
  port="$(port_for_addr "$RUNTIME_BIND")"
  runtime_bin="$ROOT_DIR/target/debug/yoi-runtime"
  if [[ ! -x "$runtime_bin" ]]; then
    printf 'runtime binary not found or not executable: %s\n' "$runtime_bin" >&2
    return 1
  fi
  start_service runtime "$ROOT_DIR" "$port" \
    env RUST_BACKTRACE="${RUST_BACKTRACE:-1}" "$runtime_bin" --bind "$RUNTIME_BIND"
}

start_backend() {
  local port backend_bin
  port="$(port_for_addr "$BACKEND_LISTEN")"
  backend_bin="$ROOT_DIR/target/debug/yoi-server"
  if [[ ! -x "$backend_bin" ]]; then
    printf 'backend binary not found or not executable: %s\n' "$backend_bin" >&2
    return 1
  fi
  start_service backend "$ROOT_DIR" "$port" \
    "$backend_bin" serve --listen "$BACKEND_LISTEN"
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
  build_runtime_backend
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
  local unit="-"
  local systemd_pid=""
  if systemd_available; then
    unit="$(systemd_unit_name "$name")"
    systemd_pid="$(systemd_main_pid "$name" || true)"
    if [[ -n "$systemd_pid" ]]; then
      managed="$systemd_pid"
    fi
  fi
  if [[ "$managed" == "-" ]] && service_pid "$name" >/dev/null; then
    managed="$(service_pid "$name")"
  fi
  mapfile -t pids < <(listener_pids_for_port "$port" || true)
  if [[ "${#pids[@]}" -gt 0 ]]; then
    listeners="${pids[*]}"
  fi
  printf '%-8s managed_pid=%-8s port=%-6s listener_pids=%-12s unit=%s\n' "$name" "$managed" "$port" "$listeners" "$unit"
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

  local stamp job_log job_unit
  stamp="$(date +%Y%m%d-%H%M%S)"
  job_log="$LOG_DIR/${action}-$stamp.job.log"
  job_unit="${UNIT_PREFIX}-${action}-${stamp}-job.service"

  log "scheduling $action in ${ACTION_DELAY_SECONDS}s; log: $job_log"
  if systemd_available; then
    systemd-run --user --unit="$job_unit" --collect --working-directory="$ROOT_DIR" \
      --property="StandardOutput=append:$job_log" \
      --property="StandardError=append:$job_log" \
      bash -c '
        set -uo pipefail
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
      ' dev-workspace-job "$ACTION_DELAY_SECONDS" "$ROOT_DIR" "$action" >/dev/null

    local scheduled_pid
    scheduled_pid="$(systemctl --user show -P MainPID "$job_unit" 2>/dev/null | awk '$1 != "" && $1 != "0" { print $1; exit }' || true)"
    printf 'scheduled_action=%s\n' "$action"
    printf 'scheduled_unit=%s\n' "$job_unit"
    printf 'scheduled_pid=%s\n' "${scheduled_pid:--}"
    printf 'scheduled_after_seconds=%s\n' "$ACTION_DELAY_SECONDS"
    printf 'scheduled_log=%s\n' "$job_log"
    return 0
  fi

  nohup setsid bash -c '
    set -uo pipefail
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
  local scheduled_pid="$!"
  disown "$scheduled_pid" 2>/dev/null || true

  printf 'scheduled_action=%s\n' "$action"
  printf 'scheduled_pid=%s\n' "$scheduled_pid"
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
