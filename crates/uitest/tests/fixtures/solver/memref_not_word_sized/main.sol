pragma solcore;

import * from std;

// `memref` has no `Typedef<word>` instance, so a reference cannot be stored as
// an array element.
function refArray() returns (uint256) {
    let refs: memref<uint256>[] = new memref<uint256>[](1);
    return refs.length;
}
