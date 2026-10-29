pragma solcore;

//~ specialize: loader std
//~ specialize: frontend clean
//~ specialize: return 1

function main(flag: bool) returns (word) {
    let x: word = 1;
    while (flag) {
        let x: word = 2;
        break;
    }
    return x;
}
