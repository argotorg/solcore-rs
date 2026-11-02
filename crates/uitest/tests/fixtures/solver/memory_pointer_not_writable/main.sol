pragma solcore;

import * from std;

// A `memory<t>` pointer is not a writable cell: element writes go through the
// checked `memref` from `lidx`. Neither a pointer built from an address nor the
// zero value of a `memory<t>[]` element can be stored through.
function overwriteFreePointer() {
    let p: memory<uint256> = memory(0x40);
    CanStore.store(p, uint256(0));
}

function writeZeroSlot() {
    let cells: memory<uint256>[] = new memory<uint256>[](1);
    CanStore.store(cells[0], uint256(5));
}
