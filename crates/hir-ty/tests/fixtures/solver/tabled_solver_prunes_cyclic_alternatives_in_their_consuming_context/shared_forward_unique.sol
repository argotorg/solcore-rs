pragma solcore;

// ambiguous-leaf: false
trait C<a> {}
trait Sub<a> where a: C {}
trait Ready<a> {}
trait Middle<a> {}
trait Shared<a> {}
impl<a> C<a> where a: Ready, a: Shared {}
impl<a> Ready<a> where a: Middle, a: Shared {}
impl<a> Middle<a> where a: C {}
impl<a> Shared<a> {}
impl<a> Sub<a> {}
trait D<a> {}
enum Box<a> {
    Box(a)
}
impl D<word> {}
impl<a> D<Box<a>> where a: D, a: C {}
