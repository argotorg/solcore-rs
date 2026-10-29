pragma solcore;

function main() returns (word) {
    let result: word;
    assembly {
        let n := calldatasize()
        mstore(0, 42)
        result := keccak256(0, n)
    }
    return result;
}

//~ sonatina: compile O0
//~ sonatina: compile O2
//~ sonatina: compile-error "dynamic length"
