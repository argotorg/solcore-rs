pragma solcore;

enum bytes16 {
    bytes16(word)
}

contract C {
    function echo(value: bytes16) public returns (bytes16) {
        return value;
    }
}
