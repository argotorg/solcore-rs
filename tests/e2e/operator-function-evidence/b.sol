pragma solcore;

import { eqWord } from std;
import * from base;
export { directB, genericB, explicitB, runtimeDirectB, runtimeGenericB };
impl Phantom<word, bool> {
    function pick(x: word) returns (bool) {
        return eqWord(x, 1);
    }
}
impl Other<word, word> {
    function other(x: word) returns (bool) {
        return eqWord(x, 0);
    }
}
function directB() returns (word) {
    let x: word = 0;
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
    let x: word = 0;
    return generic(x);
}
function explicitB() returns (word) {
    let x: word = 0;
    match (ne(x, x), lt(x, x), le(x, x), ge(x, x), and(x, x), or(x, x), not(x)) {
        case (false, false, false, false, false, false, true) {
            return 4;
        }
        case (_, _, _, _, _, _, _) {
            return 100;
        }
    }
}

function runtimeDirectB(x: word) returns (word) {
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
function runtimeGenericB(x: word) returns (word) {
    return generic(x);
}
