pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: return-variable seed

function main(flag: bool) returns (word) {
    let seed: word = 7;
    {
        if (flag) {
            let seed: word = 1;
            seed = 2;
        } else {
            seed = 3;
        }
    }
    return seed;
}
