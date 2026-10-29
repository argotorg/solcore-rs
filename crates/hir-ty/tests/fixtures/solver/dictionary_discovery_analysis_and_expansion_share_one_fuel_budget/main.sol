pragma solcore;

trait C<a> {}
trait Sub<a> where a: C {}
trait Outer<a> {}
enum Box<a> {
    Box(a)
}
impl C<word> {}
impl Sub<word> {}
impl<a> Outer<Box<a>> where a: C {}
