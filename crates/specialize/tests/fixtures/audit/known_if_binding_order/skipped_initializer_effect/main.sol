pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: no-closure-dispatch
//~ specialize: return 7

function sideEffect() returns (word) {
    assembly {
        sstore(0, 99)
    }
    return 11;
}
function main() returns (word) {
    let x: word = 1;
    if (true) {
        x = 7;
    } else {
        let unused: word = sideEffect();
    }
    let value: comptime<word> = x;
    return value;
}
