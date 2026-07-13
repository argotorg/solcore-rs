#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

profile="${PERF_PROFILE:-release}"
case_timeout="${PERF_CASE_TIMEOUT_SECONDS:-45}"
profile_dir="$profile"
if [[ "$profile" == "dev" ]]; then
  profile_dir="debug"
fi

if ! [[ "$case_timeout" =~ ^[1-9][0-9]*$ ]]; then
  echo "error: PERF_CASE_TIMEOUT_SECONDS must be a positive integer" >&2
  exit 2
fi

if [[ "${PERF_SKIP_BUILD:-0}" != "1" ]]; then
  cargo build --locked --profile "$profile" -p solcore-driver
  cargo test --locked --profile "$profile" -p solcore-vfs --lib --no-run
fi

target_dir="${CARGO_TARGET_DIR:-$repo_root/target}"
if [[ "$target_dir" != /* ]]; then
  target_dir="$repo_root/$target_dir"
fi
driver="${SOLCORE_DRIVER:-$target_dir/$profile_dir/solcore-driver}"
if [[ ! -x "$driver" ]]; then
  echo "error: compiler binary is not executable: $driver" >&2
  exit 1
fi

work_dir="$(mktemp -d "${TMPDIR:-/tmp}/solcore-perf-guard.XXXXXX")"
trap 'rm -rf "$work_dir"' EXIT

large_body="$work_dir/large-body.solc"
{
  echo 'function main() -> word {'
  echo '  let value0: word = 0;'
  for index in $(seq 1 2000); do
    previous=$((index - 1))
    echo "  let value${index}: word = value${previous};"
  done
  echo '  return value2000;'
  echo '}'
} > "$large_body"

instance_heavy="$work_dir/instance-heavy.solc"
{
  for index in $(seq 0 499); do
    echo "forall a . class a:AuditClass${index} {}"
    echo "instance word:AuditClass${index} {}"
  done
  echo 'function main() -> word { return 0; }'
} > "$instance_heavy"

run_with_deadline() {
  local name="$1"
  shift
  echo "== performance pathology guard: $name (${case_timeout}s ceiling) =="
  python3 - "$case_timeout" "$@" <<'PY'
import os
import signal
import subprocess
import sys

timeout = int(sys.argv[1])
command = sys.argv[2:]
process = subprocess.Popen(command, start_new_session=True)
try:
    returncode = process.wait(timeout=timeout)
except subprocess.TimeoutExpired:
    print(f"error: command exceeded {timeout}s: {command!r}", file=sys.stderr)
    os.killpg(process.pid, signal.SIGTERM)
    try:
        process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()
    raise SystemExit(124)
raise SystemExit(returncode)
PY
}

common_args=(
  "$driver"
  --color=never
  --unicode=never
  --diagnostic-format=short
  --warnings=never
)

run_with_deadline \
  large-body \
  "${common_args[@]}" \
  "--emit-hull=$work_dir/large-body.hull" \
  "$large_body"

run_with_deadline \
  instance-heavy \
  "${common_args[@]}" \
  "--emit-hull=$work_dir/instance-heavy.hull" \
  "$instance_heavy"

run_with_deadline \
  incremental-diagnostics \
  cargo test --locked --profile "$profile" -p solcore-vfs --lib \
  tests::incremental_diagnostics_scaling_workload -- --ignored --exact
