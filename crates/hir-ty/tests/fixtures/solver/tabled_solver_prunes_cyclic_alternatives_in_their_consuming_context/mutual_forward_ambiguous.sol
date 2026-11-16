pragma solcore;

// ambiguous-leaf: true
// std: mutual.sol
import * from std;

trait Leaf<a> {}
default impl<a> C<a> {}
default impl<a> Sub<a> {}
default impl<a> C<a> where a: Leaf {}
impl Leaf<word> {}
trait D<a> {}
enum Box<a> {
    Box(a)
}
impl D<word> {}
impl<a> D<Box<a>> where a: D, a: C {}
