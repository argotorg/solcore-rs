pragma solcore;

//~ specialize: loader std
//~ specialize: frontend clean
//~ specialize: return 1

function main() returns (word) {
    let x: word = 1;
    if (false) {
        let x: word = 2;
    } else {
        let x: word = 3;
    }
    return x;
}
