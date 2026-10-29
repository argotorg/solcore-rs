pragma solcore;

//~ layout: finite
enum Box<a> {
    Box(a)
}
enum Ignore<a> {
    Ignore
}
enum Nest<a, b> {
    Next(Ignore<Nest<Box<a>, b>>)
}
function main(x: Nest<word, bool>) returns (word) {
    return 0;
}
