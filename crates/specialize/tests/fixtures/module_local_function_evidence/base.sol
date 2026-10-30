pragma solcore;

export { C, D, run, runBoth, relay };
trait C<t> {
    function f(x: t) returns (word);
}
trait D<t> {
    function g(x: t) returns (word);
}
function run<t>(x: t) returns (word) where t: C {
    return C.f(x);
}
function runBoth<t>(x: t) returns (word) where t: C, t: D {
    return primAddWord(C.f(x), D.g(x));
}
function relay<t>(x: t) returns (word) where t: C, t: D {
    return runBoth(x);
}

export { Phantom, runPhantom };
trait Phantom<t, r> {
    function pick(x: t) returns (word);
}
function runPhantom<t, r>(x: t) returns (word) where t: Phantom<r> {
    return Phantom.pick(x);
}

export { Witness(*), Choose, First, Second, runOrdered };
enum Witness<t> {
    Witness
}
trait Choose<r, t> {
    function pick(x: t, hint: Witness<r>) returns (word);
}
trait First<t> {}
trait Second<t> {}
function runOrdered<t, r, s>(x: t, first: Witness<r>, second: Witness<s>) returns (word) where r: Choose<t>, s: Choose<t>, r: First, s: Second {
    let a = Choose.pick(x, first);
    let b = Choose.pick(x, second);
    return primAddWord(primAddWord(a, a), b);
}
