pragma solcore;

//~ specialize: loader std
//~ specialize: frontend clean
//~ specialize: return 1

import * from std;
function main() returns (word) {
    let i: word = 1;
    for (let i: word = 0; eqWord(i, 0); i = 1) {}
    return i;
}
