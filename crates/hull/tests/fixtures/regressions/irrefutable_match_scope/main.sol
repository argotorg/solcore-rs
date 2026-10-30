pragma solcore;

//~ hull: loader memory

function main() returns (word) {
    let x: word;
    assembly {
        x := calldatasize()
    }
    let y = primAddWord(x, 21);
    match (x, y) {
        case (y, x) {
            assembly {
                mstore(128, x)
            }
        }
    }
    return x;
}
