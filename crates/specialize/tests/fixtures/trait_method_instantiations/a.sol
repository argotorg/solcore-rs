pragma solcore;

import * from base;
export { allA };
enum AWord {
    AWord(word)
}
impl Local<AWord> {
    function value(x: AWord) returns (word) {
        return 1;
    }
}
impl Pick<AWord, bool> {
    function pick(x: AWord) returns (word) {
        return 1;
    }
}
function directA(x: word, y: word) returns (word) {
    return Parent.choose(x, AWord(y));
}
function valueA(x: word, y: word) returns (word) {
    let f = Parent.choose;
    return apply(f, x, AWord(y));
}
function projectedA(x: word, y: word) returns (word) {
    return throughChild(x, AWord(y));
}
function closureA(x: word, y: word) returns (word) {
    return applyOne(lam (z: word) -> word {
            return throughChild(x, AWord(z));
        }, y);
}
function derivedA(x: word, y: word) returns (word) {
    return Parent.choose(Derived(x), AWord(y));
}
function derivedValueA(x: word, y: word) returns (word) {
    let f = Parent.choose;
    return f(Derived(x), AWord(y));
}
function localA(x: word, y: word) returns (word) {
    return Parent.local(x, AWord(y));
}
function localValueA(x: word, y: word) returns (word) {
    let f = Parent.local;
    return f(x, AWord(y));
}
function localDerivedA(x: word, y: word) returns (word) {
    return Parent.local(Derived(x), AWord(y));
}
function allA(x: word, y: word) returns (word) {
    return primAddWord(primAddWord(primAddWord(primAddWord(directA(x, y), valueA(x, y)), primAddWord(projectedA(x, y), closureA(x, y))), primAddWord(derivedA(x, y), derivedValueA(x, y))), primAddWord(localA(x, y), primAddWord(localValueA(x, y), localDerivedA(x, y))));
}
