pragma solcore;

// Both paths assign the same constructor, but the variable holds no value
// before the branch.
import * from std;
import * from std.dispatch;

enum Maybe {
    Nothing,
    Just(function (word) returns (word))
}

contract C {
    function run(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let m: Maybe;
        if (x < 100) {
            m = Maybe.Just(lam (a: word) -> word {
                    return a + 1;
                });
        } else {
            m = Maybe.Just(lam (a: word) -> word {
                    return a + 2;
                });
        }
        match (m) {
            case Maybe.Just(g) {
                return uint256(g(x));
            }
            case Maybe.Nothing {
                return uint256(0);
            }
        }
    }
}
