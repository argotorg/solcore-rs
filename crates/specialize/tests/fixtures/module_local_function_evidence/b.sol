pragma solcore;

import * from base;
export { b, bothB, valueB };
impl C<word> {
    function f(x: word) returns (word) {
        return 2;
    }
}
impl D<word> {
    function g(x: word) returns (word) {
        return 20;
    }
}
function b() returns (word) {
    let x: word = 0;
    return run(x);
}
function bothB() returns (word) {
    let x: word = 0;
    return relay(x);
}
function valueB() returns (word) {
    let x: word = 0;
    let g = run;
    return g(x);
}

export { phantomB };
impl Phantom<word, bool> {
    function pick(x: word) returns (word) {
        return 4;
    }
}
function phantomB() returns (word) {
    let x: word = 0;
    return runPhantom(x);
}

export { orderedB };
impl Choose<word, word> {
    function pick(x: word, hint: Witness<word>) returns (word) {
        return 1;
    }
}
impl Choose<bool, word> {
    function pick(x: word, hint: Witness<bool>) returns (word) {
        return 2;
    }
}
impl First<bool> {}
impl Second<word> {}
function orderedB() returns (word) {
    let x: word = 0;
    let first: Witness<bool> = Witness;
    let second: Witness<word> = Witness;
    return runOrdered(x, first, second);
}
