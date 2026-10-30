pragma solcore;

function id(x: word) returns (word) {
    return x;
}
function unrelated(x: bool) returns (word) {
    return 0;
}
function main() returns (word) {
    return id(1);
}
