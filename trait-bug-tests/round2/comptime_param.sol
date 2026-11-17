pragma solcore;

trait C<t> {
    function c(x: t) returns (word);
}

enum T { T }

// The impl method demands a compile-time argument; the trait method does not.
impl C<word> {
    function c(comptime x: word) returns (word) {
        return x;
    }
}

function f(x: word) returns (word) {
    return C.c(x);
}

contract K {
    function main() public returns (word) {
        let x: word;
        assembly {
            x := calldataload(0)
        }
        return f(x);
    }
}
