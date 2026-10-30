pragma solcore;

import * from std;
import * from std.dispatch;

function capture(x: word) returns (function () returns (word)) {
    return lam () -> word {
        let out: word;
        assembly {
            out := x
        }
        return out;
    };
}

contract ReturnedClosureAssemblyCapture {
    // #[(17) -> 17]
    // #[(23) -> 23]
    function probe(input: uint256) public returns (uint256) {
        let y: word = Typedef.rep(input);
        let x: word = 99;
        let callback = capture(y);
        return uint256(callback());
    }
}
