pragma solcore;

function f(x0: word, x1: word, x2: word, x3: word, x4: word, x5: word, x6: word, x7: word, x8: word, x9: word, x10: word, x11: word, x12: word, x13: word, x14: word, x15: word) returns (word) {
    return 7;
}
function main() returns (word) {
    let g = f;
    return g(1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1);
}
