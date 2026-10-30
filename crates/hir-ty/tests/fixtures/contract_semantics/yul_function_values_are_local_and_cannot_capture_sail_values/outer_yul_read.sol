pragma solcore;

contract C {
    function main() returns (word) {
        let result: word;
        assembly {
            let outerYul := callvalue()
            function readOuter() -> value {
                value := outerYul
            }
            result := readOuter()
        }
        return result;
    }
}
