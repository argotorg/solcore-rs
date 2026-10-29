pragma solcore;

import * from base;
import * from shared;
export { allB };
impl Parent<word> {
    function f(x: word) returns (word) {
        return 2;
    }
}
impl Parent<Derived<word>> {
    function f(x: Derived<word>) returns (word) {
        return 2;
    }
}
function runB() returns (word) {
    let x: word = 0;
    return run(x);
}
function silentB() returns (word) {
    let x: word = 0;
    return runSilent(x);
}
function grandchildB() returns (word) {
    let x: word = 0;
    return runGrandchild(x);
}
function relayB() returns (word) {
    let x: word = 0;
    return relay(x);
}
function valueB() returns (word) {
    let x: word = 0;
    return value(x);
}
function methodValueB() returns (word) {
    let x: word = 0;
    return methodValue(x);
}
function nestedB() returns (word) {
    let x: word = 0;
    return runNested(Wrapped(x));
}
function derivedB() returns (word) {
    let x: word = 0;
    return run(Derived(x));
}
function directB() returns (word) {
    let x: word = 0;
    return runDirect(x);
}
function allB() returns (word) {
    return primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(0, runB()), silentB()), grandchildB()), relayB()), valueB()), methodValueB()), nestedB()), derivedB()), directB()), implicitB()), recursiveB()), derivedParentB());
}
impl ImplicitParent<word> {
    function f(x: word) returns (word) {
        return 2;
    }
}
function implicitB() returns (word) {
    let x: word = 0;
    return ImplicitChild.f(x);
}
function recursiveB() returns (word) {
    let x: word = 3;
    return ImplicitChild.g(x);
}
function derivedParentB() returns (word) {
    let x: word = 0;
    return ImplicitChild.f(ImplicitWrapped(x));
}
