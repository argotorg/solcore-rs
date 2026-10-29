pragma solcore;

contract C {
    function main() returns (word) {
        let value: word = 1;
        if (true) {
            let value: bool = true;
        } else {
            assembly {
                pop(value)
            }
        }
        assembly {
            pop(value)
        }
        return value;
    }
}
