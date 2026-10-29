pragma solcore;

//~ layout: non-regular
enum Box<a> {
    Box(a)
}
enum Nest<a> {
    Nil,
    Cons(a, (Nest<(a, a)>, word))
}
function main(x: Nest<word>) returns (word) {
    return 0;
}
