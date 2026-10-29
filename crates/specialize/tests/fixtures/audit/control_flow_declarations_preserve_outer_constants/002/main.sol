pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: return 1

function main(flag: bool) returns (word) {
    let x: word = 1;
    if (flag) {} else {
        let x: word = 7;
    }
    return x;
}
