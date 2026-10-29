pragma solcore;

contract C {
    function main() returns (word) {
        let value: word = 1;
        for (; false;) {
            let value: bool = true;
        }
        assembly {
            pop(value)
        }
        return value;
    }
}
