pragma solcore;

trait C<a> where a: Sub<word> {}
trait Seed<a> where a: C {}
trait Sub<a, r> where a: C {}
trait Fix<a, r> where a: Sub<r> {}
