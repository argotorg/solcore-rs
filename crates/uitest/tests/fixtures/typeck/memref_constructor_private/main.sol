pragma solcore;

import * from std;

// `memref` is sealed: std exports the type but not its constructor, so a
// reference cannot be built from an address.
function forge() returns (memref<uint256>) {
    return memref(0x40);
}
