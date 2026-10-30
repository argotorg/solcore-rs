pragma solcore;

function f() returns (word) {
    return 7;
}
function main() returns (word) {
    let g = f;
    return g();
}
