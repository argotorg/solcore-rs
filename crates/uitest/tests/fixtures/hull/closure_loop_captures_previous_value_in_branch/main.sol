pragma solcore;

// Only some iterations wrap the previous closure; the error points at the
// closure that captures it, not at the last assignment in the loop.
import * from std;
import * from std.dispatch;

contract C {
    function run(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a;
        };
        let g: function (word) returns (word) = lam (a: word) -> word {
            return a + 7;
        };
        let i: word = 0;
        while (i < x) {
            if (i < 2) {
                f = lam (a: word) -> word {
                    return f(a) + 1;
                };
            } else {
                f = g;
            }
            i = i + 1;
        }
        return uint256(f(x));
    }
}
