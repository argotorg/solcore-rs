pragma solcore;

trait A<a> {}
trait B<a> where a: A {}
trait C<a> where a: A {}
