pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: return 1

function compute() returns (word) {
    let x: word = 1;
    if (true) {} else {
        if (true) {
            let x: word = 7;
        } else {}
    }
    return x;
}
function main() returns (word) {
    return compute();
}
