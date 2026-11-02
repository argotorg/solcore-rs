pragma solcore;

import * from base;
export { b, bothB, valueB };
enum BWord {
    BWord(word)
}
impl C<BWord> {
    function f(x: BWord) returns (word) {
        return 2;
    }
}
impl D<BWord> {
    function g(x: BWord) returns (word) {
        return 20;
    }
}
function b() returns (word) {
    let x: BWord = BWord(0);
    return run(x);
}
function bothB() returns (word) {
    let x: BWord = BWord(0);
    return relay(x);
}
function valueB() returns (word) {
    let x: BWord = BWord(0);
    let g = run;
    return g(x);
}

export { phantomB };
impl Phantom<BWord, bool> {
    function pick(x: BWord) returns (word) {
        return 4;
    }
}
function phantomB() returns (word) {
    let x: BWord = BWord(0);
    return runPhantom(x);
}

export { orderedB };
enum BBool {
    BBool(bool)
}
impl Choose<BWord, word> {
    function pick(x: word, hint: Witness<BWord>) returns (word) {
        return 1;
    }
}
impl Choose<BBool, word> {
    function pick(x: word, hint: Witness<BBool>) returns (word) {
        return 2;
    }
}
impl First<BBool> {}
impl Second<BWord> {}
function orderedB() returns (word) {
    let x: word = 0;
    let first: Witness<BBool> = Witness;
    let second: Witness<BWord> = Witness;
    return runOrdered(x, first, second);
}
