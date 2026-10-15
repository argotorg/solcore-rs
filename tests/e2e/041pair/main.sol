import * from std;
import * from std.dispatch;

contract Pair {

    function fst(p: (word, word)) returns (word) {
        match (p) {
            case (a, b) {
                return a;
            }
        }
    }

    // #[() -> 1]
    function run() public returns (uint256) {
        return uint256(fst((1, 0)));
    }
}
