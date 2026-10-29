pragma solcore;

function and(x: word, y: word) returns (bool) {
    return true;
}

function or(x: word, y: word) returns (bool) {
    return true;
}

function conjunction(x: bool) returns (bool) {
    let lhs = .true && x;
    let rhs = x && .false;
    return lhs || rhs;
}

function disjunction(x: bool) returns (bool) {
    let lhs = .false || x;
    let rhs = x || .true;
    return lhs && rhs;
}

contract C {
    function main() public returns (bool) {
        return (.true && .false) || disjunction(conjunction(.true));
    }
}
