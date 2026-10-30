pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: no-closure-dispatch

import { allA } from a;
import { allB } from b;
function main(x: word, y: word) returns (word) {
    return primAddWord(allA(x, y), allB(x, y));
}
