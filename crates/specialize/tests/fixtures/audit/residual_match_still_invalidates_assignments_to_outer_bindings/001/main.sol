pragma solcore;

//~ specialize: loader std-current
//~ specialize: frontend clean
//~ specialize: entry-return-variable seed

function main(flag: bool) returns (word) {
    let seed: word = 7;
    match (flag) {
        case (true) {
            seed = 9;
        }
        case (false) {}
    }
    return seed;
}
