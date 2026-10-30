pragma solcore;

contract C {
    function main() returns (word) {
        let seed: comptime<word> = 7;
        let retained: word = seed;
        return retained;
    }
}
