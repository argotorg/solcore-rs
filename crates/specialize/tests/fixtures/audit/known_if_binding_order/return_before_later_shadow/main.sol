pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: no-closure-dispatch
//~ specialize: return 7

function pick() returns (word) {
    let x: word = 7;
    if (true) {
        return x;
    } else {
        let x: word = 9;
        return x;
    }
}
function main() returns (word) {
    let value: comptime<word> = pick();
    return value;
}
