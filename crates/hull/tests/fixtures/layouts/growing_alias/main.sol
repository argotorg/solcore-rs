pragma solcore;

//~ layout: non-regular
enum Box<a> {
    Box(a)
}
type Wrapped(a) = Box<a>;
enum Nest<a> {
    Nil,
    Cons(a, Nest<Wrapped<a>>)
}
function main(x: Nest<word>) returns (word) {
    return 0;
}
