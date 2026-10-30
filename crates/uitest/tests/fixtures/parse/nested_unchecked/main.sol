pragma solcore;

function f(x: word) returns (word) {
    unchecked {
        unchecked {
            return x + x;
        }
    }
}
