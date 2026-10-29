pragma solcore;

type Alias(a) = a;
trait Parent<a, b> {}
trait Child<a, b> where a: Parent<Alias<b>> {}
