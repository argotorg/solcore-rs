import * from std;
import * from std.dispatch;

function nestedSnd<a, b>(p: (a, b)) returns (b) {
    match (p) {
        case (_, tail) {
            return tail;
        }
    }
}

contract NestedPairTail {
    x: word;

    // #[() -> 42]
    function run() public returns (uint256) {
        x = 42;
        let tail = nestedSnd((x, (x, x)));
        match (tail) {
            case (head, _) {
                return uint256(head);
            }
        }
    }
}
