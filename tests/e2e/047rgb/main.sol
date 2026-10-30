pragma solcore;

import * from std;
import * from std.dispatch;

contract RGB {
    enum Color {
        R,
        G,
        B
    }
    // #[() -> 42]
    function run() public returns (uint256) {
        match (Color.B) {
            case Color.R {
                return uint256(4);
            }
            case Color.G {
                return uint256(2);
            }
            case Color.B {
                return uint256(42);
            }
        }
    }
}
