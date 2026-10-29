pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: return 9

function requireStatic(comptime x: word) returns (word) {
    return x;
}
function main() returns (word) {
    let seed: word = 7;
    {
        if (true) {
            seed = 9;
            let seed: word = 1;
        } else {}
    }
    return requireStatic(seed);
}
