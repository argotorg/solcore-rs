pragma solcore;

trait C<a> {}
trait D<a> {}
trait Sub<a> where a: C {}
trait Top<a> {}
impl<a> C<a> where a: D {}
impl<a> D<a> where a: C {}
impl<a> Sub<a> {}
impl<a> Top<a> where a: C {}
