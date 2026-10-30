pragma solcore;

//~ specialize: loader std-current
//~ specialize: frontend errors
//~ specialize: diagnostic comptime-evaluation-failed

function main(input: word) returns (word) {
    let seed: comptime<word> = input;
    return primAddWord(input, seed);
}
