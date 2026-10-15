function storeLoad(x: word) returns (word) {
    let result: word;
    assembly {
        mstore(0, x)
        result := mload(0)
    }
    return result;
}
function readMem() returns (word) {
    let result: word;
    assembly {
        result := mload(0)
    }
    return result;
}
contract C {
    function main() returns (word) {
        let seed: comptime<word> = storeLoad(42);
        assembly {
            mstore(0, 99)
        }
        let observed: comptime<word> = readMem();
        return observed;
    }
}

function writeMem() {
    assembly {
        mstore(0, 99)
    }
}

contract Wrapped {
    function main() returns (word) {
        let seed: comptime<word> = storeLoad(42);
        writeMem();
        let observed: comptime<word> = readMem();
        return observed;
    }
}

contract Loop {
    function main() returns (word) {
        let count: word;
        assembly {
            count := calldatasize()
        }
        let seed: comptime<word> = storeLoad(42);
        while (primEqWord(count, 1)) {
            // A later iteration cannot reuse memory from before the loop.
            let observed: comptime<word> = readMem();
            writeMem();
        }
        return 0;
    }
}

contract Branch {
    function main() returns (word) {
        let count: word;
        assembly {
            count := calldatasize()
        }
        let seed: comptime<word> = storeLoad(42);
        if (primEqWord(count, 1)) {
            writeMem();
        }
        let observed: comptime<word> = readMem();
        return observed;
    }
}

function writeUnknown(value: word) returns (word) {
    assembly {
        mstore(0, value)
    }
    return 0;
}
function second(first: word, value: word) returns (word) {
    return value;
}

contract Compound {
    function main() returns (word) {
        let input: word;
        assembly {
            input := calldataload(0)
        }
        let seed: comptime<word> = storeLoad(42);
        let observed: comptime<word> = second(writeUnknown(input), readMem());
        return observed;
    }
}
