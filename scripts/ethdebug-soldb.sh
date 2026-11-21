#!/usr/bin/env bash
# Steps a contract described by `solcore --ethdebug -o DIR` in SolDB.
#
#   scripts/ethdebug-soldb.sh DIR CONTRACT 'sig(types)' [ARG...] FILE:LINE
#
# First compare the programs' instruction offsets and mnemonics with
# `cast disassemble` of the .bin files. A debugger that supports
# ethdebug_bytecode.json also checks bytecode identity. Then DIR/CONTRACT.bin is deployed
# on SolDB's local chain, the function is called, execution stops at FILE:LINE,
# continues, and the source around the stop is listed.
#
# SOLDB overrides the soldb binary on PATH. Needs cast and jq.
set -euo pipefail

[[ $# -ge 4 ]] || { sed -n '4p' "$0" >&2; exit 2; }
dir=$1 contract=$2 signature=$3
shift 3
where=${!#}
args=("${@:1:$#-1}")
soldb=${SOLDB:-soldb}
# Where SolDB's local chain puts a contract deployed from its default account.
address=0x5FbDB2315678afecB367f032d93F642f64180aa3

# The instructions of PROGRAM must be the start of the disassembly of BYTES.
# `cast` names 0x44 DIFFICULTY and prints UNKNOWN(..) for unassigned bytes.
check() {
  local want have
  want=$(jq -r '.instructions[] | "\(.offset) \(.operation.mnemonic)"' "$1")
  have=$(cast disassemble "0x$(cat "$2")" | sed -E 's/DIFFICULTY/PREVRANDAO/; s/UNKNOWN\(.*\)/INVALID/' |
    while read -r offset mnemonic _; do [[ -z $offset ]] || printf '%d %s\n' "0x${offset%:}" "$mnemonic"; done)
  if [[ "$have"$'\n' != "$want"$'\n'* ]]; then
    echo "disassembly of $2 differs from $1; first difference:" >&2
    diff <(echo "$want") <(echo "$have") | head -n 5 >&2 || true
    exit 1
  fi
  echo "disassembly ok: $(wc -l <<<"$want") instructions of $1"
}
check "$dir/${contract}_ethdebug-runtime.json" "$dir/$contract.bin-runtime"
check "$dir/${contract}_ethdebug.json" "$dir/$contract.bin"

NO_COLOR=1 "$soldb" run "$dir/$contract.bin" "$signature" "${args[@]}" \
  --ethdebug-dir "$address:$contract:$dir" \
  -x "break $where" -x continue -x list --batch
