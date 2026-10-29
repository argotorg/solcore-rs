pragma solcore;

contract C {
    function main() returns (bool) {
        let value: bool = true;
        assembly {
            let value := 1
        }
        assembly {
            pop(value)
        }
        return value;
    }
}
