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
