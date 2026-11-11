pragma solcore;

export { C, Box };

trait C<a> {}
enum Box<a> {
    Box(a)
}
default impl<a> C<Box<a>> where a: C {}
