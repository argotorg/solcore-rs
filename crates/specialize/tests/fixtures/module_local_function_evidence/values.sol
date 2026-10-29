pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean

import { valueA } from a;
import { valueB } from b;
function main() returns (word) {
    return primAddWord(valueA(), valueB());
}
