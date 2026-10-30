pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: no-closure-dispatch
//~ specialize: return 9

function pick() returns (word) {
    let x: word = 7;
    {
        if (true) {
            x = 9;
        } else {
            let x: word = 1;
        }
    }
    return x;
}
function main() returns (word) {
    let value: comptime<word> = pick();
    return value;
}
