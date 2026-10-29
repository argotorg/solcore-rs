pragma solcore;

// ambiguous-leaf: false
trait C<a> {}
trait Sub<a> where a: C {}
enum Box<a> {
    Box(a)
}
impl C<word> {}
impl<a> C<Box<a>> where a: C {}
impl<a> Sub<Box<a>> where a: C {}
