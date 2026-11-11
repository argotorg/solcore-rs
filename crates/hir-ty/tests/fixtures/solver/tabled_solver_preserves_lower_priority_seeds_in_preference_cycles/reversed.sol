pragma solcore;

import * from std;

trait D<a> {}
trait Top<a> {}
impl<a> D<a> where a: C {}
default impl<a> C<a> where a: D {}
impl<a> Top<a> where a: C {}
