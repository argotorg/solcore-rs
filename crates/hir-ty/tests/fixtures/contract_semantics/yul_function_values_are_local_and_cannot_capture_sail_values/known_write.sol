pragma solcore;

contract C {
    function main() returns (word) {
        let outer: word = 7;
        assembly {
            function writeOuter() {
                outer := 9
            }
            writeOuter()
        }
        return outer;
    }
}
