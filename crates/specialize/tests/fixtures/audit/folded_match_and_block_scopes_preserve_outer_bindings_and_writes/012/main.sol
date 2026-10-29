pragma solcore;

//~ specialize: loader bare
//~ specialize: return 3

function compute() returns (word) {
    let x: word = 1;
    let z: word = 0;
    match (z) {
        case (_) {
            x = 3;
            let x: word = 4;
            x = 5;
        }
    }
    return x;
}
function main() returns (word) {
    return compute();
}
