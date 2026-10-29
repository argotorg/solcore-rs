pragma solcore;

enum Proxy<t> {}

function value() returns (Proxy<word>) {
    return @word;
}
