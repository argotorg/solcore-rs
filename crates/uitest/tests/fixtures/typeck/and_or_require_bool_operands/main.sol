pragma solcore;

function and(x: word, y: word) returns (bool) {
    return true;
}

function or(x: word, y: word) returns (bool) {
    return true;
}

function and_left(x: word) returns (bool) {
    return x && true;
}

function and_right(x: word) returns (bool) {
    return false && x;
}

function or_left(x: word) returns (bool) {
    return x || false;
}

function or_right(x: word) returns (bool) {
    return true || x;
}

function hex_operand() returns (bool) {
    return true && 0x1;
}

contract C {
    function main() public returns (bool) {
        return 1 && 2;
    }

    function other() public returns (bool) {
        return 1 || 2;
    }
}
