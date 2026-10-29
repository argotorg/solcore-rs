pragma solcore;

function a() returns (word) {
    return true;
}

function b() returns (bool) {
    return 1;
}

function c(x: word) returns (word) {
    let value: word = 1;
    return x(value);
}
