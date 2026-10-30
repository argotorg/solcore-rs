pragma solcore;

import * from std;
function main() returns (word) {
    return strlenLit(concatLit("line one
line two", "!"));
}
