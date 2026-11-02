pragma solcore;

import * from base;
export { a, bothA, valueA };
enum AWord {
    AWord(word)
}
impl C<AWord> {
    function f(x: AWord) returns (word) {
        return 1;
    }
}
impl D<AWord> {
    function g(x: AWord) returns (word) {
        return 10;
    }
}
function a() returns (word) {
    let x: AWord = AWord(0);
    return run(x);
}
function bothA() returns (word) {
    let x: AWord = AWord(0);
    return relay(x);
}
function valueA() returns (word) {
    let x: AWord = AWord(0);
    let g = run;
    return g(x);
}

export { phantomA };
impl Phantom<AWord, word> {
    function pick(x: AWord) returns (word) {
        return 3;
    }
}
function phantomA() returns (word) {
    let x: AWord = AWord(0);
    return runPhantom(x);
}

export { orderedA };
enum ABool {
    ABool(bool)
}
impl Choose<AWord, word> {
    function pick(x: word, hint: Witness<AWord>) returns (word) {
        return 1;
    }
}
impl Choose<ABool, word> {
    function pick(x: word, hint: Witness<ABool>) returns (word) {
        return 2;
    }
}
impl First<AWord> {}
impl Second<ABool> {}
function orderedA() returns (word) {
    let x: word = 0;
    let first: Witness<AWord> = Witness;
    let second: Witness<ABool> = Witness;
    return runOrdered(x, first, second);
}
