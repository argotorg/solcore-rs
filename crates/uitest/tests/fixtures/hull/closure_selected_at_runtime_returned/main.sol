pragma solcore;

// The selected function value is returned from a function that cannot be
// folded at the call site.
import * from std;
import * from std.dispatch;

function select(y: word) returns (function (word) returns (word)) {
    let f: function (word) returns (word) = lam (a: word) -> word {
        return a + 1;
    };
    if (y < 5) {
        f = lam (a: word) -> word {
            return a + 2;
        };
    }
    return f;
}

contract C {
    function run(input: uint256, choice: uint256) public returns (uint256) {
        let g: function (word) returns (word) = select(Typedef.rep(choice));
        return uint256(g(Typedef.rep(input)));
    }
}
