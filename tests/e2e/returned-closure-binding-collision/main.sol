pragma solcore;

import * from std;
import * from std.dispatch;

function capture(x: word) returns (function (word) returns (word)) {
    return lam (y: word) -> word {
        return x;
    };
}

contract CaptureCollision {
    // #[(17) -> 17]
    // #[(23) -> 23]
    function probe(input: uint256) public returns (uint256) {
        let y: word = Typedef.rep(input);
        let callback = capture(y);
        return uint256(callback(9));
    }
}
