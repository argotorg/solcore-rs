pragma solcore;

enum array<a> {
    array(word)
}
enum calldata<a> {
    calldata(word)
}

contract Fake {
    function inspect(values: calldata<array<word>>) public returns (word) {
        return 0;
    }
}
