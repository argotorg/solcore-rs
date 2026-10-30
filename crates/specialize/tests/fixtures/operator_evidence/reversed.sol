pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: return 21

import * from b;
import * from a;
function main() returns (word) {
    return primAddWord(primAddWord(directB(), directA()),
        primAddWord(primAddWord(genericB(), genericA()), primAddWord(explicitB(), explicitA())));
}
