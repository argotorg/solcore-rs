pragma solcore;

//~ specialize: loader bare
//~ specialize: return 1

function main() returns (word) {
    let x: word = 1;
    if (true) {} else {
        match (x) {
            case (x) {
                let x: word = 7;
            }
        }
    }
    return x;
}
