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
            case 0 {
                return 0;
            }
            case n {
                return n;
            }
        }
    }
}
