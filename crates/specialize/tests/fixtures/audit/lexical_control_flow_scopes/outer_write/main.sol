pragma solcore;

//~ specialize: loader std
//~ specialize: frontend clean
//~ specialize: return 9

function main() returns (word) {
    let x: word = 1;
    if (true) {
        x = 9;
        let x: word = 2;
        x = 3;
    }
    return x;
}
