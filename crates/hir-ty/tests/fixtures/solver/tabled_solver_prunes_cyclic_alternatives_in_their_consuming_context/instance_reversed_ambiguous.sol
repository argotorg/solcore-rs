pragma solcore;

// ambiguous-leaf: true
import * from std;

trait Ready<a> {}
trait Leaf<a> {}
impl Leaf<word> {}
default impl<a> C<a> where a: Leaf {}
impl<a> Ready<a> where a: C {}
default impl<a> C<a> where a: Ready {}
trait D<a> {}
enum Box<a> {
    Box(a)
}
impl D<word> {}
impl<a> D<Box<a>> where a: D, a: C {}
