pragma solcore;

pragma no-patterson-condition;

enum Box<a> {
    MkBox(a)
}

trait C<a> {
    function c(x: a) returns (word);
}

impl<a> C<a> where Box<a>: C {
    function c(x: a) returns (word) {
        return 1;
    }
}

function f() returns (word) {
    let value: word = 0;
    return C.c(value);
}
