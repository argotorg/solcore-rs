pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: return 7

import { phantomA } from a;
import { phantomB } from b;
function main() returns (word) {
    return primAddWord(phantomA(), phantomB());
}
