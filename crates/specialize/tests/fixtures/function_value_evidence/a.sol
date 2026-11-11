pragma solcore;

import * from base;
import base;
export { localA, qualifiedA, argumentA, nestedA, genericA };
enum AWord {
    AWord(word)
}
impl Phantom<AWord, word> {
    function pick(x: AWord) returns (word) {
        return 3;
    }
}
function localA() returns (word) {
    let f = run;
    let value: AWord = AWord(0);
    return f(value);
}
function qualifiedA() returns (word) {
    let f = base.run;
    let value: AWord = AWord(0);
    return f(value);
}
function argumentA() returns (word) {
    return apply(run, AWord(0));
}
function nestedA() returns (word) {
    return relay(run, AWord(0));
}
function genericA() returns (word) {
    let value: AWord = AWord(0);
    return genericValue(value);
}
