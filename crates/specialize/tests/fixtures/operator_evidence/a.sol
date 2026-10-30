pragma solcore;

import * from base;
export { directA, genericA, explicitA };
impl Phantom<word, word> {
    function pick(x: word) returns (bool) {
        return true;
    }
}
impl Other<word, bool> {
    function other(x: word) returns (bool) {
        return false;
    }
}
function directA() returns (word) {
    let x: word = 0;
    match (x != x, x < x, x <= x, x >= x,
        (x != x) && (x < x), (x <= x) || (x >= x), !x) {
        case (true, true, true, true, true, true, false) {
            return 3;
        }
        case (_, _, _, _, _, _, _) {
            return 100;
        }
    }
}
function genericA() returns (word) {
    let x: word = 0;
    return generic(x);
}
function explicitA() returns (word) {
    let x: word = 0;
    match (ne(x, x), lt(x, x), le(x, x), ge(x, x), and(x, x), or(x, x), not(x)) {
        case (true, true, true, true, true, true, false) {
            return 3;
        }
        case (_, _, _, _, _, _, _) {
            return 100;
        }
    }
}
