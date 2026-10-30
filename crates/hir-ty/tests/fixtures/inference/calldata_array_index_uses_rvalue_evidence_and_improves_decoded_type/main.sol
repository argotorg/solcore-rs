pragma solcore;

import * from std;

function inferred(xs: calldata<array<Encoded>>, i: uint256) returns () {
    let value = xs[i];
    return ();
}

function expected(xs: calldata<array<Encoded>>, i: uint256) returns (Decoded) {
    return xs[i];
}
