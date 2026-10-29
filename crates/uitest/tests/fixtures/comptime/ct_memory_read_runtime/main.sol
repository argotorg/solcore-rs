pragma solcore;

function readMem() returns (word) {
    let value: word;
    assembly {
        value := mload(0)
    }
    return value;
}

// The wrapper must not make an unresolved memory read comptime either.
function readWrapper() returns (word) {
    return readMem();
}

contract C {
    function main() returns (word) {
        let value: comptime<word> = readWrapper();
        return value;
    }
}
