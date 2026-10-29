pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean

import * from a;
import * from b;
function main() returns (word) {
    return primAddWord(
        primAddWord(primAddWord(localA(), qualifiedA()), primAddWord(argumentA(), nestedA())),
        primAddWord(primAddWord(localB(), qualifiedB()),
            primAddWord(primAddWord(argumentB(), nestedB()), primAddWord(genericA(), genericB()))));
}
