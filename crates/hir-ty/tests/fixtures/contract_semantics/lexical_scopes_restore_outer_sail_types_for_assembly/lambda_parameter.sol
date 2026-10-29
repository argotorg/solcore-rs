pragma solcore;

contract C {
    function main() returns (word) {
        let value: word = 1;
        let identity = lam (value: bool) {
            return value;
        };
        assembly {
            pop(value)
        }
        return value;
    }
}
