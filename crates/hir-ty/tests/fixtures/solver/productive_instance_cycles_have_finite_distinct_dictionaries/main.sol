pragma solcore;

trait C<a> {}
impl C<word> {}
impl<a> C<a> where a: C {}
