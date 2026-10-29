pragma solcore;

import { memory, DynArray } from std;

function filled(x: word, y: word) returns (memory<DynArray<word>>) {
    return [x, y];
}
function empty() returns (memory<DynArray<word>>) {
    return [];
}
