pragma solcore;

// ambiguous-leaf: true
import * from std;

default impl<a> C<Box<a>> where a: C {}
impl C<word> {}
impl C<word> {}
