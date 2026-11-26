pragma solcore;

import * from std;
import * from std.unsigned;

// The length of `new T[](n)` is a `uint256`, as a slice bound is.
function wordLength(n: word) returns (uint256[]) {
    return new uint256[](n);
}

function boolLength() returns (uint256[]) {
    return new uint256[](true);
}

function narrowLength(n: uint8) returns (uint256[]) {
    return new uint256[](n);
}

function nestedWordLength(n: word) returns (uint256[][]) {
    return new uint256[][](n);
}

function wordSumLength(n: word) returns (uint256[]) {
    return new uint256[](n + 1);
}
