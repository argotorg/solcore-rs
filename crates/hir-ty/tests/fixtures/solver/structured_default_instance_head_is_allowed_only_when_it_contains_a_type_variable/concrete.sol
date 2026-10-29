pragma solcore;

// invalid-default: true
enum Box<a> {
    Box(a)
}
trait Marker<a> {}
default impl Marker<Box<word>> {}
