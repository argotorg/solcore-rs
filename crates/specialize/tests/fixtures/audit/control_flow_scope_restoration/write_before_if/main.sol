pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: return 9

function requireStatic(comptime x: word) returns (word) {
    return x;
}
function main(flag: bool) returns (word) {
    let seed: word = 7;
    {
        seed = 9;
        if (flag) {
            let seed: word = 1;
        } else {}
    }
    return requireStatic(seed);
}
