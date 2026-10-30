pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: return 33

import { bothA } from a;
import { bothB } from b;
function main() returns (word) {
    return primAddWord(bothA(), bothB());
}
