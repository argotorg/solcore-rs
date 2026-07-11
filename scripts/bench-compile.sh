#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

runs="${BENCH_RUNS:-10}"
warmup="${BENCH_WARMUP:-2}"
profile="${BENCH_PROFILE:-release}"
selected_stage="${BENCH_STAGE:-}"
selected_case="${BENCH_CASE:-}"
export_dir="${BENCH_EXPORT_DIR:-}"

if ! command -v hyperfine >/dev/null 2>&1; then
  echo "error: hyperfine is required (https://github.com/sharkdp/hyperfine)" >&2
  exit 1
fi

if [[ "${BENCH_SKIP_BUILD:-0}" != "1" ]]; then
  cargo build --locked --profile "$profile" -p solcore-driver
fi

target_dir="${CARGO_TARGET_DIR:-$repo_root/target}"
if [[ "$target_dir" != /* ]]; then
  target_dir="$repo_root/$target_dir"
fi
driver="${SOLCORE_DRIVER:-$target_dir/$profile/solcore-driver}"
if [[ ! -x "$driver" ]]; then
  echo "error: compiler binary is not executable: $driver" >&2
  echo "set SOLCORE_DRIVER when Cargo uses a non-default target directory or target triple" >&2
  exit 1
fi

work_dir="$(mktemp -d "${TMPDIR:-/tmp}/solcore-compile-bench.XXXXXX")"
trap 'rm -rf "$work_dir"' EXIT

case_names=(
  "std-free"
  "dispatch-small"
  "erc20-large"
  "multi-file"
)
case_files=(
  "crates/parser/tests/fixtures/corpus/ok/test/examples/cases/SingleFun.solc"
  "tests/e2e/022add/main.solc"
  "tests/e2e/128minierc20/main.solc"
  "tests/e2e/ltimp/main.solc"
)
stages=("frontend" "hull" "yul" "sonatina" "all")

if [[ -n "$selected_stage" ]]; then
  case "$selected_stage" in
    frontend|hull|yul|sonatina|all) stages=("$selected_stage") ;;
    *)
      echo "error: BENCH_STAGE must be frontend, hull, yul, sonatina, or all" >&2
      exit 1
      ;;
  esac
fi

if [[ -n "$selected_case" ]]; then
  valid_case=0
  for case_name in "${case_names[@]}"; do
    if [[ "$selected_case" == "$case_name" ]]; then
      valid_case=1
      break
    fi
  done
  if [[ "$valid_case" != "1" ]]; then
    echo "error: BENCH_CASE must be std-free, dispatch-small, erc20-large, or multi-file" >&2
    exit 1
  fi
fi

if [[ -n "$export_dir" ]]; then
  mkdir -p "$export_dir"
fi

# Quote one argument for hyperfine's shell-words parser. Commands use
# --shell=none so the short std-free case does not measure shell startup.
quote_arg() {
  local value="$1"
  value="${value//\\/\\\\}"
  value="${value//\"/\\\"}"
  printf '"%s"' "$value"
}

command_string() {
  local command=""
  local arg
  for arg in "$@"; do
    command+="$(quote_arg "$arg") "
  done
  printf '%s' "${command% }"
}

for stage in "${stages[@]}"; do
  hyperfine_args=(--shell=none --warmup "$warmup" --runs "$runs")
  selected_count=0

  for index in "${!case_names[@]}"; do
    case_name="${case_names[$index]}"
    if [[ -n "$selected_case" && "$selected_case" != "$case_name" ]]; then
      continue
    fi

    fixture="$repo_root/${case_files[$index]}"
    output_prefix="$work_dir/$case_name"
    compiler_args=(
      "$driver"
      "--color=never"
      "--unicode=never"
      "--diagnostic-format=short"
      "--warnings=never"
    )

    case "$stage" in
      frontend) ;;
      hull) compiler_args+=("--emit-hull=$output_prefix.hull") ;;
      yul) compiler_args+=("--emit-yul=$output_prefix.yul") ;;
      sonatina) compiler_args+=("--emit-sonatina=$output_prefix.sntn") ;;
      all)
        compiler_args+=(
          "--emit-hull=$output_prefix.hull"
          "--emit-yul=$output_prefix.yul"
          "--emit-sonatina=$output_prefix.sntn"
        )
        ;;
    esac
    compiler_args+=("$fixture")

    hyperfine_args+=(
      --command-name "$case_name/$stage"
      "$(command_string "${compiler_args[@]}")"
    )
    selected_count=$((selected_count + 1))
  done

  if [[ -n "$export_dir" ]]; then
    hyperfine_args+=(--export-json "$export_dir/$stage.json")
  fi

  echo
  echo "== $stage ($selected_count fixed cases; $runs runs, $warmup warmups) =="
  hyperfine "${hyperfine_args[@]}"
done
