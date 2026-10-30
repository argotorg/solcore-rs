pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: return-variable result

function main(flag: bool) returns (word) {
    let seed: comptime<word> = 7;
    let result: word = 0;
    {
        if (flag) {
            result = seed;
            let seed: word = 1;
        } else {}
    }
    return result;
}
