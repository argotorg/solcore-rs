pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: no-closure-dispatch
//~ specialize: return 7

function c() returns (word) {
    if (true) {
        return 7;
    } else {
        let unused: word = 1;
        return 5;
    }
    return 0;
}
function consume(comptime x: word) returns (word) {
    return x;
}
function main() returns (word) {
    return consume(c());
}
