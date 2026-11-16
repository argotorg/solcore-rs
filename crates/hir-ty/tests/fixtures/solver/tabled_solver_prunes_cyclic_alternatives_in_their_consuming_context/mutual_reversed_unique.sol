pragma solcore;

// ambiguous-leaf: false
// std: mutual.sol
import * from std;

default impl<a> Sub<a> {}
default impl<a> C<a> {}
trait D<a> {}
enum Box<a> {
    Box(a)
}
impl D<word> {}
impl<a> D<Box<a>> where a: D, a: C {}
