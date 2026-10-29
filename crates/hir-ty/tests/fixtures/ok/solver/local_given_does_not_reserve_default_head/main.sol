pragma solcore;

// A fixed local assumption does not reserve a generic implementation head.
// Inside useDefault, the given for word cannot prove Value for the rigid a.
trait Value<a> {
    function value(x: a) returns (word);
}
default impl<a> Value<a> {
    function value(x: a) returns (word) {
        return 42;
    }
}
function useDefault<a>(x: a) returns (word) where word: Value {
    return Value.value(x);
}
function main() returns (word) {
    return useDefault(true);
}
