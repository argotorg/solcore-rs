pragma solcore;

enum Box<a> {
    Box(a)
}

trait C<a> {}

impl C<Box> {}
