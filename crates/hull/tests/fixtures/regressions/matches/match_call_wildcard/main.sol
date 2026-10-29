pragma solcore;

function read(x: word) returns (word) {
    let value: word;
    assembly {
        value := sload(x)
    }
    return value;
}

contract C {
    function main() public returns (word) {
        match (read(0)) {
            default {
                return 7;
            }
        }
    }
}
