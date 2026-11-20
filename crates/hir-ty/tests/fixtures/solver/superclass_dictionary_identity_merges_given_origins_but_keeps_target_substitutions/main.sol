pragma solcore;

trait A<a, r> {}
trait B<a, r> where a: A<r> {}
trait C<a, r> where a: A<r> {}
trait D<a, r> where a: B<r>, a: C<r> {}
trait E<a, r> where a: A<r> {}
trait Outer<a> {}
enum Box<a> {
    Box(a)
}
impl<a> Outer<Box<a>> where a: A<word> {}
