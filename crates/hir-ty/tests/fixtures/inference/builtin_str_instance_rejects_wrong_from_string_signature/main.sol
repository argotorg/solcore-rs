pragma solcore;

enum Wrapped {
    Wrapped(word)
}
impl Str<Wrapped> {
    function fromString(s: word) returns (Wrapped) {
        return Wrapped(s);
    }
}
