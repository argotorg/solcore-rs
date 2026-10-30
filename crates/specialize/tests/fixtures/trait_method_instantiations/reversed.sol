pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: no-closure-dispatch

import { allB } from b;
import { allA } from a;
function main(x: word, y: word) returns (word) {
    return primAddWord(allB(x, y), allA(x, y));
}
