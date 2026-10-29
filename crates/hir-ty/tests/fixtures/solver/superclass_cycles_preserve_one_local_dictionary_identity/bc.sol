pragma solcore;

trait A<a> where a: D {}
trait B<a> where a: A {}
trait C<a> where a: A {}
trait D<a> where a: B, a: C {}
