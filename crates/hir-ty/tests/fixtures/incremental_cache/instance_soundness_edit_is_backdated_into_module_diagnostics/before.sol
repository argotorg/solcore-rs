pragma solcore;

enum Box<a> {
    Box(word)
}
trait C<a, b> {}
impl<a, b> C<Box<a>, b> {}
