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
