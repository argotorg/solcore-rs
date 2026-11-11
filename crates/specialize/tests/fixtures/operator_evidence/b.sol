pragma solcore;

import * from base;
export { directB, genericB, explicitB };
enum BWord {
    BWord(word)
}
impl Phantom<BWord, bool> {
    function pick(x: BWord) returns (bool) {
        return false;
    }
}
impl Other<BWord, word> {
    function other(x: BWord) returns (bool) {
        return true;
    }
}
function directB() returns (word) {
    let x: BWord = BWord(0);
    match (x != x, x < x, x <= x, x >= x,
        (x != x) && (x < x), (x <= x) || (x >= x), !x) {
        case (false, false, false, false, false, false, true) {
            return 4;
        }
        case (_, _, _, _, _, _, _) {
            return 100;
        }
    }
}
function genericB() returns (word) {
    let x: BWord = BWord(0);
    return generic(x);
}
function explicitB() returns (word) {
    let x: BWord = BWord(0);
    match (ne(x, x), lt(x, x), le(x, x), ge(x, x), and(x, x), or(x, x), not(x)) {
        case (false, false, false, false, false, false, true) {
            return 4;
        }
        case (_, _, _, _, _, _, _) {
            return 100;
        }
    }
}
