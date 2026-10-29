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
