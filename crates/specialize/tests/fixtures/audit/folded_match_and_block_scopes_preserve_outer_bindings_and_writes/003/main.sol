pragma solcore;

//~ specialize: loader bare
//~ specialize: return 3

function main() returns (word) {
    let x: word = 1;
    let y: word = 2;
    let observed: word = 0;
    match (x, y) {
        case (y, x) {
            observed = x;
        }
    }
    return primAddWord(x, observed);
}
