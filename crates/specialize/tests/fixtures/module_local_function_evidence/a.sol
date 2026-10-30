pragma solcore;

import * from base;
export { a, bothA, valueA };
impl C<word> {
    function f(x: word) returns (word) {
        return 1;
    }
}
impl D<word> {
    function g(x: word) returns (word) {
        return 10;
    }
}
function a() returns (word) {
    let x: word = 0;
    return run(x);
}
function bothA() returns (word) {
    let x: word = 0;
    return relay(x);
}
function valueA() returns (word) {
    let x: word = 0;
    let g = run;
    return g(x);
}

export { phantomA };
impl Phantom<word, word> {
    function pick(x: word) returns (word) {
        return 3;
    }
}
function phantomA() returns (word) {
    let x: word = 0;
    return runPhantom(x);
}

export { orderedA };
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
impl First<word> {}
impl Second<bool> {}
function orderedA() returns (word) {
    let x: word = 0;
    let first: Witness<word> = Witness;
    let second: Witness<bool> = Witness;
    return runOrdered(x, first, second);
}
