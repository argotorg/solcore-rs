pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: no-closure-dispatch
//~ specialize: return 1

function main() returns (word) {
    let x: comptime<word> = 1;
    if (true) {
        let x: comptime<word> = 7;
    } else {
        let unused: word = 1;
    }
    return x;
}
