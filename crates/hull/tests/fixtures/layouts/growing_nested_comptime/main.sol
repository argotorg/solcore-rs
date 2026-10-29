pragma solcore;

//~ layout: non-regular
enum Box<a> {
    Box(a)
}
enum Nest<a> {
    Nil,
    Next(Nest<comptime<comptime<a>>>)
}
function main(x: Nest<word>) returns (word) {
    return 0;
}
