pragma solcore;

trait Fix<a, r> where a: Sub<r> {}
trait Sub<a, r> where a: C {}
trait Seed<a> where a: C {}
trait C<a> where a: Sub<word> {}
