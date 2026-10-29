pragma solcore;

function f(x0: word) returns (word) {
    return 7;
}
function main() returns (word) {
    let g = f;
    return g(1);
}
