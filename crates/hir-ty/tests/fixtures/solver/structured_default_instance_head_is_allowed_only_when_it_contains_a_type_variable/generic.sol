pragma solcore;

// invalid-default: false
enum Box<a> {
    Box(a)
}
trait Marker<a> {}
default impl<a> Marker<Box<a>> {}
