pragma solcore;

//~ specialize: loader bare
//~ specialize: max-depth 16
//~ specialize: forbid instantiation-depth-exceeded

trait Parent<t> {
    function f(x: t) returns (word);
}
trait Child<t> where t: Parent {
    function g(x: t) returns (word);
}

function decrement(n: word) returns (word) {
    let result: word;
    assembly {
        result := sub(n, 1)
    }
    return result;
}
impl Child<word> {
    function g(x: word) returns (word) {
        match (x) {
            case (0) {
                return 99;
            }
            case (_) {
                return Child.g(decrement(x));
            }
        }
    }
}
function main(n: word) returns (word) {
    return Child.g(n);
}
