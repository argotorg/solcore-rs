pragma solcore;

// ambiguous-leaf: true
import * from std;

impl C<word> {}
default impl<a> C<Box<a>> where a: C {}
impl C<word> {}
