pragma solcore;

trait Marker<a> {}
impl Marker<word> {}
#[derive(Marker)]
enum Phantom<a> {
    Phantom(word)
}
