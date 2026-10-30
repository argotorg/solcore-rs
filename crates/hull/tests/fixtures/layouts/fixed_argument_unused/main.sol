pragma solcore;

//~ layout: finite
enum Box<a> {
    Box(a)
}
enum Ignore<a> {
    Ignore
}
enum Grow<a> {
    Stop,
    Next(Grow<Box<a>>)
}
enum Root {
    Root(Ignore<Grow<word>>)
}
function main(x: Root) returns (word) {
    return 0;
}
