pragma solcore;

//~ layout: non-regular
enum Box<a> {
    Box(a)
}
enum Nest<a, b> {
    Nil,
    Left(Nest<Box<b>, word>),
    Right(Nest<word, a>)
}
function main(x: Nest<word, bool>) returns (word) {
    return 0;
}
