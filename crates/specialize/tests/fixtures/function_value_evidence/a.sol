pragma solcore;

import * from base;
import base;
export { localA, qualifiedA, argumentA, nestedA, genericA };
impl Phantom<word, word> {
    function pick(x: word) returns (word) {
        return 3;
    }
}
function localA() returns (word) {
    let f = run;
    let value: word = 0;
    return f(value);
}
function qualifiedA() returns (word) {
    let f = base.run;
    let value: word = 0;
    return f(value);
}
function argumentA() returns (word) {
    return apply(run, 0);
}
function nestedA() returns (word) {
    return relay(run, 0);
}
function genericA() returns (word) {
    let value: word = 0;
    return genericValue(value);
}
