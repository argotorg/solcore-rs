pragma solcore;

//~ specialize: loader std
//~ specialize: frontend clean
//~ specialize: return 1

function main(flag: bool) returns (word) {
    let x: word = 1;
    for (let x: word = 2; flag; x = 3) {
        let x: bool = false;
        break;
    }
    return x;
}
