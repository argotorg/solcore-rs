pragma solcore;

enum Wrap<a> {
    Wrap(a)
}

trait Foo<self, rep> {}
trait Bar<self, rep> {}

impl<a, rep> Bar<Wrap<a>, rep> where a: Foo<rep> {}

function need_bar<a, rep>(x: Wrap<a>) returns () where Wrap<a>: Bar<rep> {
    return ();
}

function use_bar<a>(x: Wrap<a>) returns () where a: Foo<word> {
    need_bar(x);
    return ();
}
