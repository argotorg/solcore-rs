pragma solcore;

//~ hull: min-count 2 "let result"
//~ hull: not-contains "$if_local"

import * from std;
function value<a>(input: a, flag: bool) returns (word) {
    let result: word = 1;
    if (flag) {
        let result: word = 7;
    } else {}
    assembly {
        result := add(result, 1)
    }
    return result;
}
function main(flag: bool) returns (word) {
    let input: word = 0;
    return primAddWord(value(input, flag), value(true, flag));
}
