pragma solcore;

import * from std;

// Array slices are read-only views, as in Solidity: writing through one is
// rejected, while reading is fine.
function write(a: uint256[]) returns (uint256) {
    let s = a[1:3];
    s[0] = uint256(7);
    s[1] += uint256(1);
    return s[0];
}

// Only collections with a `Length` instance can be sliced.
function notACollection(x: word) returns (uint256) {
    return x[1:2].length;
}

// Bounds are `uint256`.
function badBound(a: uint256[], b: bool) returns (uint256) {
    return a[b:].length;
}
