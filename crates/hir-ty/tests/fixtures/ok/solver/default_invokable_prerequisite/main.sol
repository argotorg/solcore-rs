pragma solcore;

// Defaults remain available when no structural function rule applies,
// including defaults used to satisfy another default's prerequisite.
enum Callable<a> {
    Callable(a)
}
trait Enabled<a> {}
default impl<a> Enabled<a> {}
default impl<a> invokable<Callable<a>, word, word> where a: Enabled {
    function invoke(self: Callable<a>, x: word) returns (word) {
        return x;
    }
}
function main() returns (word) {
    let callable: Callable<word> = Callable(42);
    return invokable.invoke(callable, 42);
}
