pragma solcore;

enum mapping<key, value> {
    mapping(word)
}

contract C {
    bal: mapping(word => word);

    function main() public returns (word) {
        bal = bal;
        return 7;
    }
}
