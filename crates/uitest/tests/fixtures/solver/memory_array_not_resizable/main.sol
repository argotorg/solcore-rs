pragma solcore;

import * from std;

// As in Solidity, memory arrays have a fixed length: only storage arrays
// support `push` and `pop`.
function grow(xs: uint256[], v: uint256) {
    xs.push(v);
}

function shrink(xs: uint256[]) {
    xs.pop();
}

// A struct field named `length` takes priority over the array length.
struct Span {
    length: word;
}

function spanLength(s: Span) returns (word) {
    return s.length;
}

function arrayLength(xs: uint256[]) returns (word) {
    return xs.length;
}
