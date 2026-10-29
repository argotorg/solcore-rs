pragma solcore;

import { memory, DynArray, uint256, Typedef } from std;

function read(xs: memory<DynArray<word>>, i: uint256) returns (word) {
    return xs[i];
}

function write(xs: memory<DynArray<word>>, i: uint256, value: word) returns () {
    xs[i] = value;
    return ();
}

function compound(xs: memory<DynArray<word>>, i: uint256, value: word) returns () {
    xs[i] += value;
    return ();
}

function annotatedWrite(xs: memory<DynArray<word>>, i: uint256, value: word) returns () {
    (xs[i]) = value;
    return ();
}

function annotatedCompound(xs: memory<DynArray<word>>, i: uint256, value: word) returns () {
    xs[i] += value;
    return ();
}
