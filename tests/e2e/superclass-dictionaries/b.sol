pragma solcore;

pragma no-patterson-condition;
import * from base;
import * from shared;
export { allB };
enum BWord {
    BWord(word)
}
impl Parent<BWord> {
    function f(x: BWord) returns (word) {
        return 2;
    }
}
impl Child<BWord> {
    function f(x: BWord) returns (word) {
        return 99;
    }
}
impl Silent<BWord> {}
impl Grandchild<BWord> {}
impl Direct<BWord> where BWord: Parent {
    function call(x: BWord) returns (word) {
        return Parent.f(x);
    }
}
function runB() returns (word) {
    let x: BWord = BWord(0);
    return run(x);
}
function silentB() returns (word) {
    let x: BWord = BWord(0);
    return runSilent(x);
}
function grandchildB() returns (word) {
    let x: BWord = BWord(0);
    return runGrandchild(x);
}
function relayB() returns (word) {
    let x: BWord = BWord(0);
    return relay(x);
}
function valueB() returns (word) {
    let x: BWord = BWord(0);
    return value(x);
}
function methodValueB() returns (word) {
    let x: BWord = BWord(0);
    return methodValue(x);
}
function nestedB() returns (word) {
    let x: BWord = BWord(0);
    return runNested(Wrapped(x));
}
function derivedB() returns (word) {
    let x: BWord = BWord(0);
    return run(Derived(x));
}
function directB() returns (word) {
    let x: BWord = BWord(0);
    return runDirect(x);
}
function allB() returns (word) {
    return primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(primAddWord(0, runB()), silentB()), grandchildB()), relayB()), valueB()), methodValueB()), nestedB()), derivedB()), directB()), implicitB()), recursiveB()), derivedParentB());
}
impl ImplicitParent<BWord> {
    function f(x: BWord) returns (word) {
        return 2;
    }
}
impl ImplicitChild<BWord> {
    function f(x: BWord) returns (word) {
        return ImplicitParent.f(x);
    }
    function g(x: BWord) returns (word) {
        match (x) {
            case (BWord(0)) {
                return ImplicitParent.f(x);
            }
            case (BWord(n)) {
                return ImplicitChild.g(BWord(decrement(n)));
            }
        }
    }
}
function implicitB() returns (word) {
    let x: BWord = BWord(0);
    return ImplicitChild.f(x);
}
function recursiveB() returns (word) {
    let x: BWord = BWord(3);
    return ImplicitChild.g(x);
}
function derivedParentB() returns (word) {
    let x: BWord = BWord(0);
    return ImplicitChild.f(ImplicitWrapped(x));
}
