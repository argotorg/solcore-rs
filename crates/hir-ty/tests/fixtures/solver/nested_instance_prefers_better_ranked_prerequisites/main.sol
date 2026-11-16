pragma solcore;

import * from std;

trait D<a> {}
default impl<a> C<a> {}
impl<a> D<a> where a: C {}
