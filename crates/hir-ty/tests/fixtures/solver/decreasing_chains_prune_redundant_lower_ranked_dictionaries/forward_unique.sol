pragma solcore;

// ambiguous-leaf: false
import * from std;

impl C<word> {}
default impl<a> C<Box<a>> where a: C {}
