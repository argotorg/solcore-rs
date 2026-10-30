pragma solcore;

import * from base;
import base;
export { localB, qualifiedB, argumentB, nestedB, genericB };
impl Phantom<word, bool> {
    function pick(x: word) returns (word) {
        return 4;
    }
}
function localB() returns (word) {
    let f = run;
    let value: word = 0;
    return f(value);
}
function qualifiedB() returns (word) {
    let f = base.run;
    let value: word = 0;
    return f(value);
}
function argumentB() returns (word) {
    return apply(run, 0);
}
function nestedB() returns (word) {
    return relay(run, 0);
}
function genericB() returns (word) {
    let value: word = 0;
    return genericValue(value);
}
