pragma solcore;

//~ specialize: loader bare
//~ specialize: return 2

function main() returns (word) {
    let x: word = 1;
    let y: word = 2;
    match ((x, y)) {
        case ((y, x)) {
            return x;
        }
    }
    return x;
}
