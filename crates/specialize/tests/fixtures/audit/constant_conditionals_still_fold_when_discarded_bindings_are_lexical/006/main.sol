pragma solcore;

//~ specialize: loader bare
//~ specialize: return 1

function compute() returns (word) {
    let x: word = 1;
    if (true) {
        let x: word = 7;
    } else {}
    return x;
}
function main() returns (word) {
    return compute();
}
