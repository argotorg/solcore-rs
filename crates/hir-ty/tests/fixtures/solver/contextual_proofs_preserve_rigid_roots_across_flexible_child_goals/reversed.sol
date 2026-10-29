pragma solcore;

trait C<a> {}
trait Seed<a> where a: C {}
trait Sub<a, r> where a: C {}
trait Fix<a, r> {}
impl<a, r> Sub<a, r> where a: C, a: Fix<r> {}
impl<a> Seed<a> {}
