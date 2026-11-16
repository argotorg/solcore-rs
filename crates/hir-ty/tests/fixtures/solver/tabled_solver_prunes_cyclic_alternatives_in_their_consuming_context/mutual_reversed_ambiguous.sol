pragma solcore;

// ambiguous-leaf: true
// std: mutual.sol
import * from std;

trait Leaf<a> {}
impl Leaf<word> {}
default impl<a> C<a> where a: Leaf {}
default impl<a> Sub<a> {}
default impl<a> C<a> {}
trait D<a> {}
enum Box<a> {
    Box(a)
}
impl D<word> {}
impl<a> D<Box<a>> where a: D, a: C {}
