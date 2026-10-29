pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: return 1

function compute() returns (word) {
    let x: word = 1;
    if (true) {} else {
        let x: word = 7;
    }
    return x;
}
function main() returns (word) {
    return compute();
}
