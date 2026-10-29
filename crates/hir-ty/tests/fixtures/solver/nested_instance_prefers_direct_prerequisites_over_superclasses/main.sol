pragma solcore;

trait C<a> {}
trait Sub<a> where a: C {}
trait D<a> {}
impl C<word> {}
impl Sub<word> {}
impl<a> D<a> where a: C {}
