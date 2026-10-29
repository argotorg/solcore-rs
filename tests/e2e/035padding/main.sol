pragma solcore;

import * from std;
import * from std.dispatch;

contract Option {
    enum Option<a> {
        None,
        Some(a)
    }

    function maybe(n: word, o: Option<word>) returns (word) {
        match (o) {
            case Option.Some(x) {
                return x;
            }
            case Option.None {
                return n;
            }
        }
    }

    // #[() -> 7]
    function run() public returns (uint256) {
        return uint256(maybe(7, Option.None));
    }
}
