pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean

import { allB } from b;
import { allA } from a;
function main() returns (word) {
    return primAddWord(allB(), allA());
}
