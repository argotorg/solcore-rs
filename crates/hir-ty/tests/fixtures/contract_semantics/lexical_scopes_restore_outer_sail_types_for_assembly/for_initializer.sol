pragma solcore;

contract C {
    function main() returns (word) {
        let value: word = 1;
        for (let value: bool = true; false;) {}
        assembly {
            pop(value)
        }
        return value;
    }
}
