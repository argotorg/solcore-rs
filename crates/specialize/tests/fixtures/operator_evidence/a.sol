pragma solcore;

import * from base;
export { directA, genericA, explicitA };
enum AWord {
    AWord(word)
}
impl Phantom<AWord, word> {
    function pick(x: AWord) returns (bool) {
        return true;
    }
}
impl Other<AWord, bool> {
    function other(x: AWord) returns (bool) {
        return false;
    }
}
function directA() returns (word) {
    let x: AWord = AWord(0);
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
    let x: AWord = AWord(0);
    return generic(x);
}
function explicitA() returns (word) {
    let x: AWord = AWord(0);
    match (ne(x, x), lt(x, x), le(x, x), ge(x, x), and(x, x), or(x, x), not(x)) {
        case (true, true, true, true, true, true, false) {
            return 3;
        }
        case (_, _, _, _, _, _, _) {
            return 100;
        }
    }
}
