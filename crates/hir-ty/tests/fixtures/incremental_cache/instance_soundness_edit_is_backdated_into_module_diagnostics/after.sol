pragma solcore;

enum Box<a> {
    Box(word)
}
trait C<a, b> {}
impl<a> C<Box<a>, word> {}
