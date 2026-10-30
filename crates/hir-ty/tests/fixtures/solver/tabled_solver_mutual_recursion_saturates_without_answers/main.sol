pragma solcore;

trait C<a> {}
trait D<a> {}

impl<a> C<a> where a: D {}
impl<a> D<a> where a: C {}
