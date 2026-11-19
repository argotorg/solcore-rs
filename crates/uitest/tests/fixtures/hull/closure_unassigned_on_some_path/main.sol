pragma solcore;

// Only one path gives the declared function variable a value.
import * from std;
import * from std.dispatch;

contract C {
    function run(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word);
        if (x < 100) {
            f = lam (a: word) -> word {
                return a + 1;
            };
        }
        return uint256(f(x));
    }
}
