pragma solcore;

//~ specialize: loader std
//~ specialize: frontend clean
//~ specialize: return 1

function pick() returns (word) {
    let x: word = 1;
    if (true) {
        let x: word = 2;
    }
    return x;
}
function main() returns (word) {
    let x: comptime<word> = pick();
    return x;
}
