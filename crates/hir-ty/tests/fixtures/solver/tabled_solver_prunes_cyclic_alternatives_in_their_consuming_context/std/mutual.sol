pragma solcore;

export { C, Sub };

trait C<a> {}
trait Sub<a> {}
default impl<a> C<a> where a: Sub {}
default impl<a> Sub<a> where a: C {}
