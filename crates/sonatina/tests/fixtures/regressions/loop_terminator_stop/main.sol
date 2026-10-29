pragma solcore;

function main() returns (word) {
    assembly {
        for {
            let i := 0
        } lt(i, calldatasize()) {
            i := add(i, 1)
        } {
            stop()
        }
    }
    return 0;
}

//~ sonatina: valid
