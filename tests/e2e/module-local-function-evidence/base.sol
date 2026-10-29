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
