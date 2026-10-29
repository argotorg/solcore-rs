pragma solcore;

import { memory, DynArray } from std;

function mixed(x: word, flag: bool) returns (memory<DynArray<word>>) {
    return [x, flag];
}
