pragma solcore;

contract C {
    i: word;

    function main() public returns (word) {
        for (let i: word; false;) {}
        return i;
    }
}
