pragma solcore;

enum Wrap<a> {
    Wrap(a)
}

trait Foo<self, rep> {}
trait Bar<self, rep> {}

impl<a, rep> Bar<Wrap<a>, rep> where a: Foo<rep> {}
