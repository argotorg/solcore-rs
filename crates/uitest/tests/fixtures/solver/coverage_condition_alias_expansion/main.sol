pragma solcore;

type Phantom(a) = word;
trait MyClass<a, b> {}

impl<a> MyClass<Phantom<a>, a> {}
