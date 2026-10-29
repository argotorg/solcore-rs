pragma solcore;

import { RValueIdxAccess } from std;

enum calldata<t> {
    calldata(word)
}
enum array<t> {
    array(word)
}

function read(xs: calldata<array<word>>, i: word) returns (word) {
    return xs[i];
}
