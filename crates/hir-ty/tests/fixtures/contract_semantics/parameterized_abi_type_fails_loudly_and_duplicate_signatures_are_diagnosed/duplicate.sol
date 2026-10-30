pragma solcore;

contract Dup {
    function f(x: word) public returns (word) {
        return x;
    }
    function f(x: word) public returns (word) {
        return x;
    }
}
