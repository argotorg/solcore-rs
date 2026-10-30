pragma solcore;

import * from std;

function write(xs: calldata<array<Encoded>>, i: uint256, value: Decoded) {
    xs[i] = value;
    return ();
}

function compound(xs: calldata<array<Encoded>>, i: uint256, value: Decoded) {
    xs[i] += value;
    return ();
}

function annotatedWrite(xs: calldata<array<Encoded>>, i: uint256, value: Decoded) {
    (xs[i]) = value;
    return ();
}

function annotatedCompound(xs: calldata<array<Encoded>>, i: uint256, value: Decoded) {
    xs[i] += value;
    return ();
}
