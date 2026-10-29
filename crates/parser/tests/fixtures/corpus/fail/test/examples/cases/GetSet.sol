pragma solcore;

contract GetSet {
    value: Word;

    function setValue(x: Word) public {
        value = x;
    }

    function getValue() public returns (Word) {
        return value;
    }
}
