pragma solcore;

// The constructor that holds the function value depends on a run-time branch.
import * from std;
import * from std.dispatch;

enum Maybe {
    Nothing,
    Just(function (word) returns (word))
}

contract C {
    function run(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let m: Maybe = Maybe.Nothing;
        if (x < 100) {
            m = Maybe.Just(lam (a: word) -> word {
                    return a + 1;
                });
        }
        match (m) {
            case Maybe.Just(_) {
                return uint256(1);
            }
            case Maybe.Nothing {
                return uint256(0);
            }
        }
    }
}
