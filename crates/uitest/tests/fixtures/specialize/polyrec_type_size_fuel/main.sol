pragma solcore;

function go<a>(x: a) returns (word) {
    return go((x, x));
}

contract C {
    function main() public returns (word) {
        let x: word = 1;
        return go(x);
    }
}
