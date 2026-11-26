pragma solcore;

export { Marker, marked, Box, Phantom };

trait Marker<a> {}

impl Marker<word> {}

function marked<a>(x: a) returns (word) where a: Marker {
    return 1;
}

#[derive(Marker)]
enum Box<a> {
    Box(a)
}

#[derive(Marker)]
enum Phantom<a> {
    Phantom(word)
}
