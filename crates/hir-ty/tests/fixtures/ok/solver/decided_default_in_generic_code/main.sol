pragma solcore;

// Generic code may rely on a default impl when every default that could take
// precedence over it is decided: it applies here, or it can never apply.
import * from std;

trait Q<t> {}
trait B<t> where t: Q {}

enum Foo {
    Foo
}

enum Bar {
    Bar
}

impl Q<Foo> {}
impl B<Foo> {}

enum Box<a> {
    Box(a)
}

enum Wrap<a> {
    Wrap(a)
}

// Takes precedence over std's `default impl<self> StorageSize<self>` wherever
// `a: Q` holds.
default impl<a> StorageSize<Box<a>> where a: Q {
    function size(x: Proxy<Box<a>>) returns (word) {
        return 2;
    }
}

// Its condition mentions no type parameter and does not hold, so it never
// applies.
default impl<a> StorageSize<Wrap<a>> where Bar: Q {
    function size(x: Proxy<Wrap<a>>) returns (word) {
        return 3;
    }
}

function sizeOf<v>(x: v) returns (word) where v: StorageSize {
    let p: Proxy<v> = Proxy;
    return StorageSize.size(p);
}

// The caller decides the stated constraint.
function stated<u>(x: u) returns (word) where Box<u>: StorageSize {
    return sizeOf(Box(x));
}

// The assumption decides the condition, so the default above applies.
function assumed<u>(x: u) returns (word) where u: Q {
    return sizeOf(Box(x));
}

// So does an assumption whose trait has the condition as a superclass.
function viaSuperclass<u>(x: u) returns (word) where u: B {
    return sizeOf(Box(x));
}

// std's default decides `Wrap<u>: StorageSize`, as it does for every `Wrap`.
function refuted<u>(x: u) returns (word) {
    return sizeOf(Wrap(x));
}

// An impl's superclass only has to hold for every instantiation: which
// default provides it is chosen again for the instantiated impl.
trait Stored<t> where t: StorageSize {}
impl<a> Stored<Box<a>> {}

function main() returns (word) {
    return stated(Foo.Foo) + assumed(Foo.Foo) + viaSuperclass(Foo.Foo) + refuted(Foo.Foo);
}
