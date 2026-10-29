pragma solcore;

function main_a() returns (word) {
    let x: word;
    assembly {
        x := calldatasize()
    }
    return x;
}
function main() returns (word) {
    return primAddWord(main_a(), 42);
}

//~ sonatina: valid
