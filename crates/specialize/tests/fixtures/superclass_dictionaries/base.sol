pragma solcore;

import { Generic } from std.Generic;
export { Parent, Child, Silent, Grandchild, Wrapped(*), UsesChild, run, runSilent, runGrandchild, relay, value, methodValue, runNested, Derived(*), Direct, runDirect, ImplicitParent, ImplicitChild, ImplicitWrapped(*) };
trait Parent<t> {
    function f(x: t) returns (word);
}
trait Child<t> where t: Parent {
    function f(x: t) returns (word);
}
trait Silent<t> where t: Parent {}
trait Grandchild<t> where t: Silent {}
enum Wrapped<t> {
    Wrapped(t)
}
trait UsesChild<t> {
    function run(x: t) returns (word);
}
function run<t>(x: t) returns (word) where t: Child {
    return Parent.f(x);
}
function runSilent<t>(x: t) returns (word) where t: Silent {
    return Parent.f(x);
}
function runGrandchild<t>(x: t) returns (word) where t: Grandchild {
    return Parent.f(x);
}
function relay<t>(x: t) returns (word) where t: Child {
    return run(x);
}
function value<t>(x: t) returns (word) where t: Child {
    let f = run;
    return f(x);
}
function methodValue<t>(x: t) returns (word) where t: Child {
    let f = Parent.f;
    return f(x);
}
function runNested<t>(x: t) returns (word) where t: UsesChild {
    return UsesChild.run(x);
}

#[derive(Child)]
enum Derived<t> {
    Derived(t)
}
trait Direct<t> where t: Parent {
    function call(x: t) returns (word);
}
function runDirect<t>(x: t) returns (word) where t: Direct {
    return Direct.call(x);
}

trait ImplicitParent<t> {
    function f(x: t) returns (word);
}
trait ImplicitChild<t> where t: ImplicitParent {
    function f(x: t) returns (word);
    function g(x: t) returns (word);
}
#[derive(ImplicitParent)]
enum ImplicitWrapped<t> {
    ImplicitWrapped(t)
}
