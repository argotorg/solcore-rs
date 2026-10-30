pragma solcore;

// ambiguous-leaf: true
trait C<a> {}
trait Sub<a> where a: C {}
trait Ready<a> {}
impl<a> C<a> where a: Ready {}
impl<a> Ready<a> where a: C {}
impl<a> Sub<a> {}
impl C<word> {}
trait D<a> {}
enum Box<a> {
    Box(a)
}
impl D<word> {}
impl<a> D<Box<a>> where a: D, a: C {}
