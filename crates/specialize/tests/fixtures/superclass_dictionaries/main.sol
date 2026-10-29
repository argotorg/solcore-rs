pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean

import { allA } from a;
import { allB } from b;
function main() returns (word) {
    return primAddWord(allA(), allB());
}
