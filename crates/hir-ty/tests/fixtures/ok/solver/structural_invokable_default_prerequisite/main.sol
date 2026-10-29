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

enum Wrapped<a> {
    Wrapped(a)
}
trait Callable<a> {}
impl<f> Callable<Wrapped<f>> where f: invokable<word, word> {}

function accept<a>(value: a) returns (word) where a: Callable {
    return 42;
}
function identity(x: word) returns (word) {
    return x;
}
function main() returns (word) {
    let f: function (word) returns (word) = identity;
    return accept(Wrapped(f));
}
