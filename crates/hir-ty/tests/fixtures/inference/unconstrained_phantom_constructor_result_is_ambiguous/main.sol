pragma solcore;

enum Foo<a> {
    Foo(word)
}

function read<a>(x: Foo<a>) returns (word) {
    return 0;
}

function main() returns (word) {
    return read(Foo(42));
}
