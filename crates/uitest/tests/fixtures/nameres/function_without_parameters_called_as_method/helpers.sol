pragma solcore;

export { one, twice };

function one() returns (word) {
    return 1;
}

function twice(x: word) returns (word) {
    return x;
}
