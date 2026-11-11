pragma solcore;

import * from base;
import base;
export { localB, qualifiedB, argumentB, nestedB, genericB };
enum BWord {
    BWord(word)
}
impl Phantom<BWord, bool> {
    function pick(x: BWord) returns (word) {
        return 4;
    }
}
function localB() returns (word) {
    let f = run;
    let value: BWord = BWord(0);
    return f(value);
}
function qualifiedB() returns (word) {
    let f = base.run;
    let value: BWord = BWord(0);
    return f(value);
}
function argumentB() returns (word) {
    return apply(run, BWord(0));
}
function nestedB() returns (word) {
    return relay(run, BWord(0));
}
function genericB() returns (word) {
    let value: BWord = BWord(0);
    return genericValue(value);
}
