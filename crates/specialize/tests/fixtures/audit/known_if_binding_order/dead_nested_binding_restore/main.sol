pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: no-closure-dispatch
//~ specialize: return 7

function pick() returns (word) {
    let x: word = 7;
    {
        if (false) {
            if (true) {
                let x: word = 9;
            }
        } else {}
    }
    return x;
}
function main() returns (word) {
    let value: comptime<word> = pick();
    return value;
}
