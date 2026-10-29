pragma solcore;

contract C {
    function main() returns (word) {
        let result: word;
        assembly {
            let i := 0
            for {} lt(i, 1) {
                i := leaked
            } {
                let leaked := 1
                i := add(i, 1)
            }
            result := i
        }
        return result;
    }
}
