pragma solcore;

import * from std;
import * from std.dispatch;

contract C {
    function capturing(value: uint256) public returns (uint256) {
        let x: word = Typedef.rep(value);
        let first: function () returns (word) = lam () -> word {
            let result: word;
            assembly {
                result := x
            }
            return result;
        };
        assembly {
            x := add(x, 7)
        }
        let second: function () returns (word) = lam () -> word {
            let result: word;
            assembly {
                result := x
            }
            return result;
        };
        return uint256(primAddWord(first(), second()));
    }

    function plain(value: uint256) public returns (uint256) {
        let y: word = Typedef.rep(value);
        let f: function () returns (word) = lam () -> word {
            return 5;
        };
        return uint256(primAddWord(y, f()));
    }
}
