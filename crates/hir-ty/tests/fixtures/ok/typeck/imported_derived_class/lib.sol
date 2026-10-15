export { Marker, Box, Phantom };

trait Marker<a> {
    function mark(x: a) returns (word);
}

impl Marker<word> {
    function mark(x: word) returns (word) {
        return x;
    }
}

#[derive(Marker)]
enum Box<a> {
    Box(a)
}

#[derive(Marker)]
enum Phantom<a> {
    Phantom(word)
}
