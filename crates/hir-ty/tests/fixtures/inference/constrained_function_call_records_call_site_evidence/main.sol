pragma solcore;

enum T {
    T
}

trait C<a> {}
impl C<T> {}

function use<a>(x: a) returns (word) where a: C {
    return 0;
}

function main(t: T) returns (word) {
    return use(t);
}
