pragma solcore;

trait C<a> {}
impl<a> C<a> where a: C {}
