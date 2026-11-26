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

large_body="$work_dir/large-body.sol"
{
  echo 'pragma solcore;'
  echo 'function main() returns (word) {'
  echo '  let value0: word = 0;'
  for index in $(seq 1 2000); do
    previous=$((index - 1))
    echo "  let value${index}: word = value${previous};"
  done
  echo '  return value2000;'
  echo '}'
} > "$large_body"

instance_heavy="$work_dir/instance-heavy.sol"
{
  echo 'pragma solcore;'
  for index in $(seq 0 499); do
    echo "trait AuditClass${index}<a> {}"
    echo "impl AuditClass${index}<word> {}"
  done
  echo 'function main() returns (word) { return 0; }'
} > "$instance_heavy"

# Source references in assembly must not retain a copy of all visible Yul
# locals at every occurrence. Interleave declarations with both reads and
# writes so an accidental scope snapshot grows quadratically.
assembly_references="$work_dir/assembly-references.sol"
{
  echo 'pragma solcore;'
  echo 'function main() returns (word) {'
  echo '  let result: word = 0;'
  echo '  assembly {'
  for index in $(seq 0 1999); do
    echo "    let value${index} := ${index}"
    echo "    result := add(result, value${index})"
  done
  echo '  }'
  echo '  return result;'
  echo '}'
} > "$assembly_references"

# A nested ADT has a linear-sized runtime representation. Check both expression
# inference and backend layout traversal, keeping a runtime value through the
# constructors and matches so the workload is not just dead constant data.
python3 - "$work_dir" <<'PY'
from pathlib import Path
import sys

directory = Path(sys.argv[1])

def box_type(level):
    return "Box<" * level + "word" + ">" * level

for style, depth in [("sequential", 32), ("inline", 24)]:
    # The inline case remains below the existing expression-depth limit.
    unwrap = [
        "enum Box<a> { Box(a) }",
        f"function unwrap(value: {box_type(depth)}) returns(word) {{",
    ]
    for level in range(depth, 0, -1):
        value = "value" if level == depth else f"value{level}"
        unwrap.append(f"match ({value}) {{ case (Box(value{level - 1})) {{")
    unwrap.extend(["return value0;", "}}" * depth, "}"])

    lines = ["pragma solcore;"] + unwrap + [
        "function main() returns(word) {",
        "let value: word; assembly { value := calldataload(0) }",
    ]
    if style == "sequential":
        lines.append("let value0: word = value;")
        for level in range(1, depth + 1):
            lines.append(
                f"let value{level}: {box_type(level)} = Box(value{level - 1});"
            )
        lines.append(f"return unwrap(value{depth});")
    else:
        value = "Box(" * depth + "value" + ")" * depth
        lines.append(f"return unwrap({value});")
    lines.append("}")
    (directory / f"nested-adt-{style}.sol").write_text("\n".join(lines) + "\n")
PY

# A subtrait call in a free function, with a recursive user impl of the std
# superclass (issue #170). The backend's frontend desugar plan solves the call
# in a trait environment built without imports, where every clause naming
# `StorageSize` lowers to an error predicate. Error predicates once proved
# each other, so the recursive impl wrapped its own answers without end.
subtrait_recursive_impl="$work_dir/subtrait-recursive-impl.sol"
cat > "$subtrait_recursive_impl" <<'SOLCORE'
pragma solcore;
import * from std;
import * from std.dispatch;

enum Foo {
    Foo
}

enum Wrap<a> {
    Wrap(a)
}

impl<a> StorageSize<Wrap<a>> where a: StorageSize {
    function size(x: Proxy<Wrap<a>>) returns (word) {
        let p: Proxy<a> = Proxy;
        return primAddWord(StorageSize.size(p), 100);
    }
}

trait R<t> where t: StorageSize {
    function r(x: t) returns (word);
}

impl<a> R<Wrap<a>> where a: StorageSize {
    function r(x: Wrap<a>) returns (word) {
        return 7;
    }
}

function site() returns (word) {
    return R.r(Wrap(Foo.Foo));
}

contract C {
    function run() public returns (uint256) {
        return uint256(site());
    }
}
SOLCORE

# A chain of generic functions that each wrap their argument once more, under
# a constraint whose conditional impl has a superclass (issue #175). The
# dictionary of every level once held a second copy of the level below, in
# the recipe of its superclass, so each level doubled the work.
python3 - "$work_dir" <<'PY'
from pathlib import Path
import sys

depth = 24
lines = [
    "pragma solcore;",
    "import * from std;",
    "import * from std.dispatch;",
    "enum Box<a> { Box(a) }",
    "impl<a> Eq<Box<a>> where a: Eq {",
    "    function eq(x: Box<a>, y: Box<a>) returns (bool) { return true; }",
    "}",
    "impl<a> Ord<Box<a>> where a: Ord {",
    "    function gt(x: Box<a>, y: Box<a>) returns (bool) { return true; }",
    "}",
]
for level in range(depth):
    if level + 1 < depth:
        body = f"return wrap{level + 1}(Box(x));"
    else:
        body = "return Eq.eq(x, x);"
    lines.append(
        f"function wrap{level}<p>(x: p) returns (bool) where p: Ord {{ {body} }}"
    )
lines += [
    "contract C {",
    "    function run(a: uint256) public returns (bool) { return wrap0(a); }",
    "}",
]
path = Path(sys.argv[1], "conditional-superclass-chain.sol")
path.write_text("\n".join(lines) + "\n")
PY

run_with_limits() {
  local name="$1"
  shift
  echo "== performance pathology guard: $name (${case_timeout}s ceiling) =="
  python3 - "$case_timeout" "${PERF_CASE_MAX_RSS_MIB:-0}" "$@" <<'PY'
import os
import resource
import signal
import subprocess
import sys

timeout = int(sys.argv[1])
max_rss_mib = int(sys.argv[2])
command = sys.argv[3:]
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
if returncode == 0 and max_rss_mib:
    # Each guard runs in a fresh Python process with one child. macOS reports
    # peak RSS in bytes; Linux reports KiB. Check after exit rather than imposing
    # a virtual-address-space limit, which is not portable between allocators.
    rss = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
    rss_mib = rss / (1024 * 1024 if sys.platform == "darwin" else 1024)
    print(f"peak RSS: {rss_mib:.1f} MiB ({max_rss_mib} MiB ceiling)")
    if rss_mib > max_rss_mib:
        print(f"error: command exceeded memory ceiling: {command!r}", file=sys.stderr)
        raise SystemExit(1)
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

run_with_limits \
  large-body \
  "${common_args[@]}" \
  "--emit-hull=$work_dir/large-body.hull" \
  "$large_body"

run_with_limits \
  instance-heavy \
  "${common_args[@]}" \
  "--emit-hull=$work_dir/instance-heavy.hull" \
  "$instance_heavy"

# Frontend-only resolution normally needs well below this generous ceiling.
# The former per-reference snapshots consumed roughly 700 MiB for this input.
PERF_CASE_MAX_RSS_MIB=256 run_with_limits \
  assembly-references \
  "${common_args[@]}" \
  "$assembly_references"

for style in sequential inline; do
  run_with_limits \
    "nested-adt-$style" \
    "${common_args[@]}" \
    "--emit-yul=$work_dir/nested-adt-$style.yul" \
    "--emit-sonatina=$work_dir/nested-adt-$style.snt" \
    "$work_dir/nested-adt-$style.sol"
done

run_with_limits \
  subtrait-recursive-impl \
  "${common_args[@]}" \
  "--std-root=$repo_root/std" \
  "--emit-hull=$work_dir/subtrait-recursive-impl.hull" \
  "$subtrait_recursive_impl"

run_with_limits \
  conditional-superclass-chain \
  "${common_args[@]}" \
  "--std-root=$repo_root/std" \
  "--emit-hull=$work_dir/conditional-superclass-chain.hull" \
  "$work_dir/conditional-superclass-chain.sol"

run_with_limits \
  incremental-diagnostics \
  cargo test --locked --profile "$profile" -p solcore-vfs --lib \
  tests::incremental_diagnostics_scaling_workload -- --ignored --exact
