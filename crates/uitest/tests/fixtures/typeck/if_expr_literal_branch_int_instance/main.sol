pragma solcore;

import * from std;

struct Meters {
    value: uint256;
}

// uint256 has an `Int` instance, so the literal branch takes its type.
function accepted(c: bool, a: uint256) returns (uint256) {
    let x = c ? a : 0;
    return x;
}

function boolBranch(c: bool, b: bool) returns (bool) {
    let x = c ? b : 0;
    return x;
}

function userTypeBranch(c: bool, m: Meters) returns (Meters) {
    let x = c ? 1 : m;
    return x;
}

function unconstrainedParameter<t>(c: bool, a: t) returns (t) {
    return c ? a : 0;
}
