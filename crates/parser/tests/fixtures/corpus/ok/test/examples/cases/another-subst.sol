pragma solcore;

trait Foo<a> {
    function foo(x: a);
}

impl<a, b> Foo<(a, b)> where a: Foo, b: Foo {
    function foo(p: (a, b)) {
        match (p) {
            case (pa, pb) {
                Foo.foo(pa);
                Foo.foo(pb);
            }
        }
    }
}
