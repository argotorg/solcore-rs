pragma solcore;

pragma no-patterson-condition;
import * from base;
import * from shared;
export { allA };
enum AWord {
    AWord(word)
}
impl Parent<AWord> {
    function f(x: AWord) returns (word) {
        return 1;
    }
}
impl Child<AWord> {
    function f(x: AWord) returns (word) {
        return 99;
    }
}
impl Silent<AWord> {}
impl Grandchild<AWord> {}
impl Direct<AWord> where AWord: Parent {
    function call(x: AWord) returns (word) {
        return Parent.f(x);
    }
}
function runA() returns (word) {
    let x: AWord = AWord(0);
    return run(x);
}
function silentA() returns (word) {
    let x: AWord = AWord(0);
    return runSilent(x);
}
function grandchildA() returns (word) {
    let x: AWord = AWord(0);
    return runGrandchild(x);
}
function relayA() returns (word) {
    let x: AWord = AWord(0);
    return relay(x);
}
function valueA() returns (word) {
    let x: AWord = AWord(0);
    return value(x);
}
function methodValueA() returns (word) {
    let x: AWord = AWord(0);
    return methodValue(x);
}
function nestedA() returns (word) {
    let x: AWord = AWord(0);
    return runNested(Wrapped(x));
}
function derivedA() returns (word) {
    let x: AWord = AWord(0);
    return run(Derived(x));
}
function directA() returns (word) {
    let x: AWord = AWord(0);
    return runDirect(x);
}
function allA() returns (word) {
    return primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(0, runA()), silentA()), grandchildA()), relayA()), valueA()), methodValueA()), nestedA()), derivedA()), directA()), implicitA()), recursiveA()), derivedParentA());
}
impl ImplicitParent<AWord> {
    function f(x: AWord) returns (word) {
        return 1;
    }
}
impl ImplicitChild<AWord> {
    function f(x: AWord) returns (word) {
        return ImplicitParent.f(x);
    }
    function g(x: AWord) returns (word) {
        match (x) {
            case (AWord(0)) {
                return ImplicitParent.f(x);
            }
            case (AWord(n)) {
                return ImplicitChild.g(AWord(decrement(n)));
            }
        }
    }
}
function implicitA() returns (word) {
    let x: AWord = AWord(0);
    return ImplicitChild.f(x);
}
function recursiveA() returns (word) {
    let x: AWord = AWord(3);
    return ImplicitChild.g(x);
}
function derivedParentA() returns (word) {
    let x: AWord = AWord(0);
    return ImplicitChild.f(ImplicitWrapped(x));
}
