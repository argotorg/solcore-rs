pragma solcore;

// Each iteration wraps the previous closure in a new one, so the number of
// nested closures depends on the run-time trip count.
import * from std;
import * from std.dispatch;

contract C {
    function run(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a;
        };
        let i: word = 0;
        while (i < x) {
            f = lam (a: word) -> word {
                return f(a) + 1;
            };
            i = i + 1;
        }
        return uint256(f(x));
    }
}
