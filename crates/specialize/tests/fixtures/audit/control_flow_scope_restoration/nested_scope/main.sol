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
        let seed: word = 11;
        if (flag) {
            let seed: word = 1;
        } else {}
    }
    return requireStatic(seed);
}
