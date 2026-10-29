pragma solcore;

trait C<a, b> {}
trait Sub<a, b> where a: C<b> {}
enum Box<a> {
    Box(a)
}
impl<a, b> C<Box<a>, Box<b>> where b: C<a> {}
impl<a, b> Sub<Box<a>, Box<b>> where b: C<a> {}
