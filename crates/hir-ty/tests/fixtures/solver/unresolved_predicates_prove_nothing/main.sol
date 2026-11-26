pragma solcore;

// `Missing` is not declared, so both impl heads below and every condition
// naming it lower to error predicates. The second impl would wrap the first
// in itself forever if failed predicates could prove each other.
enum Foo {
    Foo
}

enum Wrap<a> {
    Wrap(a)
}

trait R<t> {}

impl Missing<Foo> {}
impl<a> Missing<Wrap<a>> where a: Missing {}
impl<a> R<Wrap<a>> where a: Missing {}
