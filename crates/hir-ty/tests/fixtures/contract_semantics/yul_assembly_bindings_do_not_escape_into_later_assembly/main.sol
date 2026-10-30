pragma solcore;

contract C {
    function main() returns (word) {
        assembly {
            let temporary := 1
        }
        assembly {
            pop(temporary)
        }
        return 0;
    }
}
