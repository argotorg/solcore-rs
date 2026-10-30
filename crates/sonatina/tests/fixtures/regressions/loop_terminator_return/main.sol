pragma solcore;

function main() returns (word) {
    assembly {
        for {
            let i := 0
        } lt(i, calldatasize()) {
            i := add(i, 1)
        } {
            return (0, 0)
        }
    }
    return 0;
}

//~ sonatina: valid
