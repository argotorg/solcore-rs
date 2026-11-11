pragma solcore;

import * from base;
export { allB };
enum BWord {
    BWord(word)
}
impl Local<BWord> {
    function value(x: BWord) returns (word) {
        return 2;
    }
}
impl Pick<BWord, word> {
    function pick(x: BWord) returns (word) {
        return 2;
    }
}
function directB(x: word, y: word) returns (word) {
    return Parent.choose(x, BWord(y));
}
function valueB(x: word, y: word) returns (word) {
    let f = Parent.choose;
    return apply(f, x, BWord(y));
}
function projectedB(x: word, y: word) returns (word) {
    return throughChild(x, BWord(y));
}
function closureB(x: word, y: word) returns (word) {
    return applyOne(lam (z: word) -> word {
            return throughChild(x, BWord(z));
        }, y);
}
function derivedB(x: word, y: word) returns (word) {
    return Parent.choose(Derived(x), BWord(y));
}
function derivedValueB(x: word, y: word) returns (word) {
    let f = Parent.choose;
    return f(Derived(x), BWord(y));
}
function localB(x: word, y: word) returns (word) {
    return Parent.local(x, BWord(y));
}
function localValueB(x: word, y: word) returns (word) {
    let f = Parent.local;
    return f(x, BWord(y));
}
function localDerivedB(x: word, y: word) returns (word) {
    return Parent.local(Derived(x), BWord(y));
}
function allB(x: word, y: word) returns (word) {
    return primAddWord(primAddWord(primAddWord(primAddWord(directB(x, y), valueB(x, y)), primAddWord(projectedB(x, y), closureB(x, y))), primAddWord(derivedB(x, y), derivedValueB(x, y))), primAddWord(localB(x, y), primAddWord(localValueB(x, y), localDerivedB(x, y))));
}
