pragma solcore;

//~ layout: non-regular
enum Box<a> {
    Box(a)
}
enum Nest<a> {
    Nil,
    Cons(Box<Box<Nest<Box<a>>>>)
}
function main(x: Nest<word>) returns (word) {
    return 0;
}
