pragma solcore;

import * from base;
export { allB };
impl Local<word> {
    function value(x: word) returns (word) {
        return 2;
    }
}
impl Pick<word, word> {
    function pick(x: word) returns (word) {
        return 2;
    }
}
function directB(x: word, y: word) returns (word) {
    return Parent.choose(x, y);
}
function valueB(x: word, y: word) returns (word) {
    let f = Parent.choose;
    return apply(f, x, y);
}
function projectedB(x: word, y: word) returns (word) {
    return throughChild(x, y);
}
function closureB(x: word, y: word) returns (word) {
    return applyOne(lam (z: word) -> word {
            return throughChild(x, z);
        }, y);
}
function derivedB(x: word, y: word) returns (word) {
    return Parent.choose(Derived(x), y);
}
function derivedValueB(x: word, y: word) returns (word) {
    let f = Parent.choose;
    return f(Derived(x), y);
}
function localB(x: word, y: word) returns (word) {
    return Parent.local(x, y);
}
function localValueB(x: word, y: word) returns (word) {
    let f = Parent.local;
    return f(x, y);
}
function localDerivedB(x: word, y: word) returns (word) {
    return Parent.local(Derived(x), y);
}
function allB(x: word, y: word) returns (word) {
    return primAddWord(primAddWord(primAddWord(primAddWord(directB(x, y), valueB(x, y)), primAddWord(projectedB(x, y), closureB(x, y))), primAddWord(derivedB(x, y), derivedValueB(x, y))), primAddWord(localB(x, y), primAddWord(localValueB(x, y), localDerivedB(x, y))));
}
