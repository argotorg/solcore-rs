pragma solcore;

import * from std;
function main() returns (word) {
    return strlenLit(concatLit("ab", "!"));
}

//~ sonatina: contains "return 4.i256"
//~ sonatina: file-url-imports
