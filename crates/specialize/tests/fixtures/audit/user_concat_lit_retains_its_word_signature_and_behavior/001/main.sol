pragma solcore;

//~ specialize: loader bare
//~ specialize: return 7

function concatLit(x: word) returns (word) {
    return x;
}
function main() returns (word) {
    return concatLit(7);
}
