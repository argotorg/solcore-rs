pragma solcore;

import * from std;

trait D<a> {}
trait Top<a> {}
default impl<a> C<a> where a: D {}
impl<a> D<a> where a: C {}
impl<a> Top<a> where a: C {}
