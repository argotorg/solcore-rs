pragma solcore;

// A skipped default must not force its non-terminating prerequisite.
pragma no-patterson-condition;
pragma no-coverage-condition;
pragma no-bounded-variable-condition;

enum Box<a> {
    Box(a)
}
trait Growing<a> {}
impl<a> Growing<a> where Box<a>: Growing {}

default impl<f, args> invokable<f, args, word> where f: Growing {
    function invoke(self: f, args: args) returns (word) {
        return 0;
    }
}

function nullary() returns (word) {
    return 42;
}
function unary(x: word) returns (word) {
    return x;
}
function wide(a: word, b: word, c: word, d: word, e: word, f: word, g: word, h: word, i: word, j: word, k: word, l: word, m: word, n: word, o: word, p: word) returns (word) {
    return p;
}

function checkNullary() returns (word) {
    let f: function () returns (word) = nullary;
    return f();
}
function checkUnary() returns (word) {
    let f: function (word) returns (word) = unary;
    return f(42);
}
function checkWide() returns (word) {
    let f: function (word, word, word, word, word, word, word, word, word, word, word, word, word, word, word, word) returns (word) = wide;
    return f(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 42);
}
