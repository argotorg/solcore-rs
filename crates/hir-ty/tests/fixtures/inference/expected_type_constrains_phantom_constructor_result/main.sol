pragma solcore;

enum Foo<a> {
    Foo(word)
}

function main() returns (Foo<word>) {
    return Foo(42);
}
