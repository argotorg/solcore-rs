pragma solcore;

trait C<a> {
    function c(x: a) returns (word);
}

function bad<a>() returns (word) where a: C {
    let value: word = 1;
    return C.c(value);
}
