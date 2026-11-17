#!/usr/bin/env bash
# Checks the shape of the ethdebug files that `solcore --ethdebug -o DIR` wrote.
#
#   scripts/ethdebug-shape.sh DIR
#
# Every program must have exactly the keys ethdebug, compilation, contract,
# environment and instructions; instructions only offset, operation and context;
# contexts only code and remark. Offsets must increase from 0, and every source
# range must lie inside the text of a source listed in ethdebug_resources.json.
# Prints `shape ok: <programs> programs, <instructions> instructions`.
# Needs jq 1.7 or later. This is not a validation against the JSON schemas.
set -euo pipefail

dir=${1:?usage: ethdebug-shape.sh DIR}
resources="$dir/ethdebug_resources.json"
fail() { echo "shape error: $*" >&2; exit 1; }
[[ -f "$resources" ]] || fail "$resources is missing"

shopt -s nullglob
programs=("$dir"/*_ethdebug.json "$dir"/*_ethdebug-runtime.json)
[[ ${#programs[@]} -gt 0 ]] || fail "no program files in $dir"

jq -e --slurpfile compilation "$dir/ethdebug_compilation.json" '
  ((keys | sort) == ["compilation", "ethdebug", "pointers", "types"])
  and (.ethdebug == {schema: "ethdebug/format/info/resources", version: "0.1.0-draft.2"})
  and .compilation == $compilation[0]
  and ([.compilation.sources[] | (keys | sort) == ["contents", "id", "language", "path"]] | all)
' "$resources" > /dev/null || fail "$resources or ethdebug_compilation.json has the wrong keys"

total=0
for file in "${programs[@]}"; do
  environment=call
  [[ $file == *_ethdebug.json ]] && environment=create
  count=$(jq -r --slurpfile resources "$resources" --arg environment "$environment" '
    def need(cond; msg): if cond then . else error(msg) end;
    ($resources[0].compilation.sources
      | map({key: (.id | tostring), value: (.contents | utf8bytelength)}) | from_entries) as $size
    | def inside(r): ($size[r.source.id | tostring]) as $n
        | need($n != null; "source \(r.source.id) is not listed in the resources")
        | need(r.range.offset + r.range.length <= $n; "range \(r.range) is outside source \(r.source.id)");
      need((keys | sort) == ["compilation", "contract", "environment", "ethdebug", "instructions"];
           "program keys are \(keys)")
    | need(.environment == $environment; "environment is \(.environment), not \($environment)")
    | need(.ethdebug == {schema: "ethdebug/format/program", version: "0.1.0-draft.2"}; "wrong stamp")
    | inside(.contract.definition)
    | [.instructions[].offset] as $offsets
    | need(($offsets | length) == 0 or $offsets[0] == 0; "offsets do not start at 0")
    | need([range(1; $offsets | length) | $offsets[.] > $offsets[. - 1]] | all;
           "offsets do not strictly increase")
    | (.instructions[]
        | need((keys - ["offset", "operation", "context"]) == []; "instruction keys are \(keys)")
        | need(.operation.mnemonic != null; "instruction at \(.offset) has no mnemonic")
        | (.context // {})
        | need((keys - ["code", "remark"]) == []; "context keys are \(keys)")
        | if .code then inside(.code) else . end
        | empty),
      (.instructions | length)
  ' "$file") || fail "$file"
  total=$((total + count))
done

echo "shape ok: ${#programs[@]} programs, $total instructions"
