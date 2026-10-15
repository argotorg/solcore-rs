function sloadWord() returns (word) {
    let v: word;
    assembly {
        v := sload(0)
    }
    return v;
}

contract C {
    function main() public returns (word) {
        let y: comptime<word> = sloadWord();
        return y;
    }
}
