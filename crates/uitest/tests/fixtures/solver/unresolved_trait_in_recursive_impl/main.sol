pragma solcore;

enum Foo {
    Foo
}

enum Wrap<a> {
    Wrap(a)
}

impl Missing<Foo> {
    function size(x: Foo) returns (word) {
        return 1;
    }
}

impl<a> Missing<Wrap<a>> where a: Missing {
    function size(x: Wrap<a>) returns (word) {
        return 2;
    }
}

trait R<t> {
    function r(x: t) returns (word);
}

impl<a> R<Wrap<a>> where a: Missing {
    function r(x: Wrap<a>) returns (word) {
        return 7;
    }
}

function site() returns (word) {
    return R.r(Wrap(Foo.Foo));
}
