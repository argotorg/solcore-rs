pragma solcore;

// ambiguous-leaf: false
import * from std;

trait Ready<a> {}
impl<a> Ready<a> where a: C {}
default impl<a> C<a> where a: Ready {}
trait D<a> {}
enum Box<a> {
    Box(a)
}
impl D<word> {}
impl<a> D<Box<a>> where a: D, a: C {}
