pragma solcore;

type W = word;

function f(x: W) returns (W) {
    x
}

contract C {

    function main() public returns (word) {
        return f(42);
    }
}
