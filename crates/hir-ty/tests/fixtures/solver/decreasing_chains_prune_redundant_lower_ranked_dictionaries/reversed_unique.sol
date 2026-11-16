pragma solcore;

// ambiguous-leaf: false
import * from std;

default impl<a> C<Box<a>> where a: C {}
impl C<word> {}
