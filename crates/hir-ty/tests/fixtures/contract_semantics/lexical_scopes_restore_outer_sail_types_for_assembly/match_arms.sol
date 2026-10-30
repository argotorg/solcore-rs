pragma solcore;

contract C {
    function main() returns (word) {
        let value: word = 1;
        match (true) {
            case true {
                let value: bool = true;
            }
            case false {
                assembly {
                    pop(value)
                }
            }
        }
        assembly {
            pop(value)
        }
        return value;
    }
}
