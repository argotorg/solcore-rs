pragma solcore;

enum Wrapped {
    Wrapped(word)
}
impl Str<Wrapped> {
    function unexpected(x: word) returns (word) {
        return x;
    }
}
