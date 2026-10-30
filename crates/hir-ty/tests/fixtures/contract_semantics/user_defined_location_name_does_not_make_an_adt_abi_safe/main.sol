pragma solcore;

enum memory<a> {
    memory(word)
}
enum Wrap {
    Wrap(memory<(word, bool)>)
}

contract Shapes {
    function roundtrip(value: Wrap) public returns (Wrap) {
        return value;
    }
}
