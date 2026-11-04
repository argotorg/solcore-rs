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
