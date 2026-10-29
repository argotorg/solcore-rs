pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: no-closure-dispatch
//~ specialize: return 7

function main() returns (word) {
    let x: word = 1;
    if (true) {
        x = 7;
    } else {
        let unused: comptime<word> = 9;
    }
    let value: comptime<word> = x;
    return value;
}
