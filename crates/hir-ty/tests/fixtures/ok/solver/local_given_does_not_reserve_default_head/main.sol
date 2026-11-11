pragma solcore;

// A fixed local assumption does not reserve a generic implementation head.
// Inside useDefault, the given for Box<word> cannot prove Value for the rigid
// Box<a>, so the default impl decides that goal.
enum Box<a> {
    Box(a)
}
trait Value<a> {
    function value(x: a) returns (word);
}
default impl<a> Value<a> {
    function value(x: a) returns (word) {
        return 42;
    }
}
function useDefault<a>(x: Box<a>) returns (word) where Box<word>: Value {
    return Value.value(x);
}
function main() returns (word) {
    return useDefault(Box(true));
}
