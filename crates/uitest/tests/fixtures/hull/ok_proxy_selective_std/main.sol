pragma solcore;

import { uint256 } from std;

function identity<t>(value: t) returns (t) {
    return value;
}

contract SelectiveWitness {
    function main() returns (word) {
        let witness = identity(@word);
        return 42;
    }
}
