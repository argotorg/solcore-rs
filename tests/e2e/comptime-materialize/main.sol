pragma solcore;

import * from std;
import * from std.dispatch;

struct Words {
    first: word;
    second: word;
}

contract ComptimeMaterialize {
    // A known comptime number becomes a runtime word at each use.
    // #[() -> 7]
    function numeric() public returns (uint256) {
        let seed: comptime<word> = 7;
        let value: word = seed;
        return uint256(value);
    }

    // #[() -> 42]
    function hex() public returns (uint256) {
        let seed: comptime<word> = 0x2a;
        return uint256(seed);
    }

    // Materialize the known field without changing the runtime field's type.
    // #[(23) -> 30]
    function mixed(input: uint256) public returns (uint256) {
        let seed: comptime<word> = 7;
        let words = Words(seed, Typedef.rep(input));
        return uint256(words.first) + uint256(words.second);
    }
}
