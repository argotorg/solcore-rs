pragma solcore;

// ambiguous-leaf: false
trait C<a> where a: Sub {}
trait Sub<a> where a: C {}
impl<a> C<a> {}
impl<a> Sub<a> {}
trait D<a> {}
enum Box<a> {
    Box(a)
}
impl D<word> {}
impl<a> D<Box<a>> where a: D, a: C {}
