pragma solcore;

trait C<a, r> where a: Sub<r> {}
trait Sub<a, r> where a: C<r> {}
trait Wanted<a, r> {}
trait Outer<a> {}
enum Box<a> {
    Box(a)
}
impl Sub<word, bool> {}
impl C<word, word> {}
impl Wanted<word, bool> {}
impl<a, r> Outer<Box<a>> where a: Wanted<r>, a: C<r> {}
