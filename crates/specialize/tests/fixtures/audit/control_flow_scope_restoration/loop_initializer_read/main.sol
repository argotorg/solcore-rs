pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: return-variable result

function main(flag: bool) returns (word) {
    let seed: comptime<word> = 7;
    let result: word = 0;
    {
        for (let seed: word = seed; flag;) {
            result = seed;
            break;
        }
    }
    return result;
}
