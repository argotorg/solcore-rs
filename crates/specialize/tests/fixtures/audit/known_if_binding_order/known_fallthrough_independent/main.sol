pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: no-closure-dispatch
//~ specialize: return 7

function c() returns (comptime<word>) {
    if (true) {
        let selected: word = 7;
    } else {
        let unused: word = 1;
    }
    return 7;
}
function main() returns (word) {
    return c();
}
