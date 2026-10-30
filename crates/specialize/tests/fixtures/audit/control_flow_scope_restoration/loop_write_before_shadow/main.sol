pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: return-variable seed

function requireStatic(comptime x: word) returns (word) {
    return x;
}
function main(flag: bool) returns (word) {
    let seed: word = 7;
    {
        for (; flag; seed = 9) {
            let seed: word = 1;
            break;
        }
    }
    return seed;
}
