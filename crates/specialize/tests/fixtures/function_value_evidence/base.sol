pragma solcore;

export { Phantom, run, apply, relay, genericValue };
trait Phantom<t, r> {
    function pick(x: t) returns (word);
}
function run<t, r>(x: t) returns (word) where t: Phantom<r> {
    return Phantom.pick(x);
}
function apply<t>(f: function (t) returns (word), x: t) returns (word) {
    return f(x);
}
function relay<t>(f: function (t) returns (word), x: t) returns (word) {
    return apply(f, x);
}
function genericValue<t, r>(x: t) returns (word) where t: Phantom<r> {
    let f = run;
    return f(x);
}
