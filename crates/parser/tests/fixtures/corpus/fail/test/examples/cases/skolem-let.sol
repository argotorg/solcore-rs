function fromWord<a>(x: word) returns (a) {
    let result: a;
    assembly {
        result := x
    }
    return result;
}

contract Unsafe {
    function main() public returns (word) {
        fromWord(7);
        return 42;
    }
}
