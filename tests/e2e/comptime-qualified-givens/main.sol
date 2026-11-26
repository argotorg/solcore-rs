pragma solcore;

import * from std;
import * from std.dispatch;

// Instance selection sees through `comptime`, but a dictionary is staged for
// the qualifiers of its predicate. A given is the dictionary of a goal that
// spells the same predicate; a goal that differs in `comptime` has its impl
// selected when the function is specialized.
trait A<t> {
    function fa(x: t) returns (word);
}

trait B<t> where t: A {
    function fb(x: t) returns (word);
}

trait C<t> where t: A {
    function fc(x: t) returns (word);
}

enum Foo {
    Foo
}

enum Wrap<a> {
    Wrap(a)
}

enum Box<a> {
    Box(a)
}

impl A<Foo> {
    function fa(x: Foo) returns (word) {
        return 1;
    }
}

impl C<Foo> {
    function fc(x: Foo) returns (word) {
        return 2;
    }
}

impl<a> A<Wrap<a>> where a: A {
    function fa(x: Wrap<a>) returns (word) {
        match (x) {
            case Wrap(inner) {
                return primAddWord(A.fa(inner), 10);
            }
        }
    }
}

impl<a> C<Wrap<a>> where a: C {
    function fc(x: Wrap<a>) returns (word) {
        return 20;
    }
}

impl<a> A<Box<a>> where a: A {
    function fa(x: Box<a>) returns (word) {
        match (x) {
            case Box(inner) {
                return primAddWord(A.fa(inner), 100);
            }
        }
    }
}

// The superclass `Box<a>: A` needs `a: A`, which the condition `a: C` gives.
impl<a> B<Box<a>> where a: C {
    function fb(x: Box<a>) returns (word) {
        return 30;
    }
}

function viaB<v>(x: v) returns (word) where v: B {
    return A.fa(x);
}

function boxed<t>(x: t) returns (word) where t: C {
    return viaB(Box(x));
}

trait Qualified<t> {
    function value(x: t) returns (word);
}

impl<a> Qualified<a> {
    function value(x: a) returns (word) {
        return 7;
    }
}

// The assumption is written with the qualifier of the parameter.
function direct<t>(comptime x: t) returns (word) where comptime<t>: Qualified {
    return Qualified.value(x);
}

enum Pair<a, b> {
    Pair(a, b)
}

impl<x> A<Pair<comptime<x>, x>> {
    function fa(p: Pair<comptime<x>, x>) returns (word) {
        return 1000;
    }
}

// The head carries the qualifier, and so does the superclass that the method
// body assumes.
impl<x> B<Pair<comptime<x>, x>> {
    function fb(p: Pair<comptime<x>, x>) returns (word) {
        return A.fa(p);
    }
}

function needsComptime<t>(comptime x: t) returns (word) {
    return 5;
}

trait Staged<t> {
    function staged(x: t) returns (word);
}

trait Refined<t> where t: Staged {
    function refined(x: t) returns (word);
}

// The method passes its argument on as a compile-time value, so it exists
// only for a qualified argument type.
impl<a> Staged<a> {
    function staged(x: a) returns (word) {
        return needsComptime(x);
    }
}

impl<a> Refined<a> {
    function refined(x: a) returns (word) {
        return 2;
    }
}

// `t: Refined` provides `t: Staged`, staged for a run-time `t`. The goal is
// `comptime<t>: Staged`: the impl is selected for it.
function stagedForGoal<t>(comptime x: t) returns (word) where t: Refined {
    return Staged.staged(x);
}

// Two givens reach `A`, and the goal differs from both in `comptime`.
function twoSubtraits<t>(comptime x: t) returns (word) where t: B, t: C {
    return A.fa(x);
}

// The givens are qualified and the goal is not.
function qualifiedSubtraits<t>(x: t) returns (word) where comptime<t>: B, comptime<t>: C {
    return A.fa(x);
}

impl B<Foo> {
    function fb(x: Foo) returns (word) {
        return 3;
    }
}

contract ComptimeQualifiedGivens {
    // #[() -> 111]
    function plain() public returns (uint256) {
        let x: Wrap<Foo> = Wrap(Foo.Foo);
        return uint256(boxed(x));
    }

    // #[() -> 111]
    function qualified() public returns (uint256) {
        let x: comptime<Wrap<comptime<Foo>>> = Wrap(Foo.Foo);
        return uint256(boxed(x));
    }

    // #[() -> 7]
    function qualifiedAssumption() public returns (uint256) {
        let x: comptime<Box<comptime<word>>> = Box(3);
        return uint256(direct(x));
    }

    // #[() -> 1000]
    function qualifiedHead() public returns (uint256) {
        let p: comptime<Pair<comptime<word>, word>> = Pair(1, 2);
        return uint256(B.fb(p));
    }

    // #[() -> 5]
    function staging() public returns (uint256) {
        let x: comptime<word> = 3;
        return uint256(stagedForGoal(x));
    }

    // #[() -> 1]
    function qualifiedGoal() public returns (uint256) {
        let x: comptime<Foo> = Foo.Foo;
        return uint256(twoSubtraits(x));
    }

    // #[() -> 1]
    function qualifiedGivens() public returns (uint256) {
        return uint256(qualifiedSubtraits(Foo.Foo));
    }
}
