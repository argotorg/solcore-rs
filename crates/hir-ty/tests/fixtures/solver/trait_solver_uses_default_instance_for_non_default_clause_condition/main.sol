pragma solcore;

enum Wrap<a> {
    Wrap(a)
}

trait DefaultDependency<a> {}
default impl<a> DefaultDependency<a> {}

trait Outer<a> {}
impl<a> Outer<Wrap<a>> where a: DefaultDependency {}
