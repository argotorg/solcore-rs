pragma solcore;

//~ layout: non-regular
enum Box<a> {
    Box(a)
}
type Qualified(a) = comptime<a>;
enum Nest<a> {
    Nil,
    Next(Nest<Qualified<a>>)
}
function main(x: Nest<word>) returns (word) {
    return 0;
}
