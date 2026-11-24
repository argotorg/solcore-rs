pragma solcore;

function double(x: word) returns (word) {
    return x + x;
}

function main() returns (word) {
    let x: word = 1;
    return x.double();
}
