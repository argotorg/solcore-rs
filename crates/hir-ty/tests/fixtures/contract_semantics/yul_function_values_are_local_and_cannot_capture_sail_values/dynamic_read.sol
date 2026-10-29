pragma solcore;

contract C {
    function main() returns (word) {
        let outer: word;
        assembly {
            outer := callvalue()
            function readOuter() -> value {
                value := outer
            }
            outer := readOuter()
        }
        return outer;
    }
}
