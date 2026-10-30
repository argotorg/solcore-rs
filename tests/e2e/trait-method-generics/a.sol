pragma solcore;

import * from base;
export { allA };
impl Local<word> {
    function value(x: word) returns (word) {
        return 1;
    }
}
impl Pick<word, bool> {
    function pick(x: word) returns (word) {
        return 1;
    }
}
function directA(x: word, y: word) returns (word) {
    return Parent.choose(x, y);
}
function valueA(x: word, y: word) returns (word) {
    let f = Parent.choose;
    return apply(f, x, y);
}
function projectedA(x: word, y: word) returns (word) {
    return throughChild(x, y);
}
function closureA(x: word, y: word) returns (word) {
    return applyOne(lam (z: word) -> word {
            return throughChild(x, z);
        }, y);
}
function derivedA(x: word, y: word) returns (word) {
    return Parent.choose(Derived(x), y);
}
function derivedValueA(x: word, y: word) returns (word) {
    let f = Parent.choose;
    return f(Derived(x), y);
}
function localA(x: word, y: word) returns (word) {
    return Parent.local(x, y);
}
function localValueA(x: word, y: word) returns (word) {
    let f = Parent.local;
    return f(x, y);
}
function localDerivedA(x: word, y: word) returns (word) {
    return Parent.local(Derived(x), y);
}
function allA(x: word, y: word) returns (word) {
    return primAddWord(primAddWord(primAddWord(primAddWord(directA(x, y), valueA(x, y)), primAddWord(projectedA(x, y), closureA(x, y))), primAddWord(derivedA(x, y), derivedValueA(x, y))), primAddWord(localA(x, y), primAddWord(localValueA(x, y), localDerivedA(x, y))));
}
