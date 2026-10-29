pragma solcore;

import * from base;
import * from shared;
export { allA };
impl Parent<word> {
    function f(x: word) returns (word) {
        return 1;
    }
}
impl Parent<Derived<word>> {
    function f(x: Derived<word>) returns (word) {
        return 1;
    }
}
function runA() returns (word) {
    let x: word = 0;
    return run(x);
}
function silentA() returns (word) {
    let x: word = 0;
    return runSilent(x);
}
function grandchildA() returns (word) {
    let x: word = 0;
    return runGrandchild(x);
}
function relayA() returns (word) {
    let x: word = 0;
    return relay(x);
}
function valueA() returns (word) {
    let x: word = 0;
    return value(x);
}
function methodValueA() returns (word) {
    let x: word = 0;
    return methodValue(x);
}
function nestedA() returns (word) {
    let x: word = 0;
    return runNested(Wrapped(x));
}
function derivedA() returns (word) {
    let x: word = 0;
    return run(Derived(x));
}
function directA() returns (word) {
    let x: word = 0;
    return runDirect(x);
}
function allA() returns (word) {
    return primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(0, runA()), silentA()), grandchildA()), relayA()), valueA()), methodValueA()), nestedA()), derivedA()), directA()), implicitA()), recursiveA()), derivedParentA());
}
impl ImplicitParent<word> {
    function f(x: word) returns (word) {
        return 1;
    }
}
function implicitA() returns (word) {
    let x: word = 0;
    return ImplicitChild.f(x);
}
function recursiveA() returns (word) {
    let x: word = 3;
    return ImplicitChild.g(x);
}
function derivedParentA() returns (word) {
    let x: word = 0;
    return ImplicitChild.f(ImplicitWrapped(x));
}
