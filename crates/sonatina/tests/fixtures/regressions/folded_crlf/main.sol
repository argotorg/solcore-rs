pragma solcore;

import * from std;
function main() returns (word) {
    return strlenLit(concatLit("line one
line two", "!"));
}

//~ sonatina: contains "return 19.i256"
//~ sonatina: file-url-imports
