pragma solcore;

import { Eq, Ord, absurd } from std;

function eqUnit(x: (), y: ()) returns (bool) {
    return Eq.eq(x, y);
}

function eqSum(x: sum<word, word>, y: sum<word, word>) returns (bool) {
    return Eq.eq(x, y);
}

function eqProduct(x: (word, word), y: (word, word)) returns (bool) {
    return Eq.eq(x, y);
}

function ordUnit(x: (), y: ()) returns (bool) {
    return Ord.gt(x, y);
}

function ordSum(x: sum<word, word>, y: sum<word, word>) returns (bool) {
    return Ord.gt(x, y);
}

function ordProduct(x: (word, word), y: (word, word)) returns (bool) {
    return Ord.gt(x, y);
}

function bottomWord() returns (word) {
    return absurd();
}
