pragma solcore;

trait Conv<t, r> {
    function conv(x: t) returns (r);
}

trait Sup<t, r> where t: Conv<r> {
    function sup(x: t) returns (r);
}

trait Conv2<t, r> {
    function conv2(x: t, y: r) returns (word);
}

enum Foo {
    Foo
}

impl Conv2<Foo, word> {
    function conv2(x: Foo, y: word) returns (word) {
        return y;
    }
}

impl<a> Conv<a, word> {
    function conv(x: a) returns (word) {
        return 7;
    }
}

impl<a> Sup<a, word> {
    function sup(x: a) returns (word) {
        return 8;
    }
}

// Both givens prove `t: Conv<r>` with the same `r`, so the result of
// `Conv.conv` has type `r`, as with `where t: Conv<r>` alone.
function f<t, r>(x: t) returns (word) where t: Conv<r>, t: Sup<r> {
    let y = Conv.conv(x);
    return Conv2.conv2(Foo.Foo, y);
}
