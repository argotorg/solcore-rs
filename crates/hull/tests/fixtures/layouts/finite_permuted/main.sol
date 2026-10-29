pragma solcore;

//~ layout: unsupported-recursive
enum Box<a> {
    Box(a)
}
enum Nest<a, b> {
    Nil,
    Next(a, Nest<b, a>)
}
function main(x: Nest<word, bool>) returns (word) {
    return 0;
}
