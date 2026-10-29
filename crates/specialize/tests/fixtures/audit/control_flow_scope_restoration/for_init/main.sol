pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: return 7

function requireStatic(comptime x: word) returns (word) {
    return x;
}
function main(flag: bool) returns (word) {
    let seed: comptime<word> = 7;
    {
        for (let seed: word = 1; flag; seed = 2) {
            seed = 3;
            break;
        }
    }
    return requireStatic(seed);
}
