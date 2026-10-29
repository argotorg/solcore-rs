pragma solcore;

contract C {
    function main() returns (word) {
        let result: word;
        assembly {
            function localValue(input) -> output {
                let temporary := input
                output := temporary
            }
            result := localValue(7)
        }
        return result;
    }
}
