import * from std;
import * from std.dispatch;

type Tail = (uint256, uint256);

function keep<a>(value: (uint256, a)) returns ((uint256, a)) {
    return value;
}

function aliasTail(value: (uint256, Tail)) returns (uint256) {
    match (value) {
        case (x, y, z) {
            return x + y + z;
        }
    }
}

contract TupleOpenTail {
    // #[() -> 42]
    function flat() public returns (uint256) {
        let value = keep((uint256(12), uint256(13), uint256(17)));
        return aliasTail(value);
    }

    // #[() -> 42]
    function nested() public returns (uint256) {
        return aliasTail(keep((uint256(12), (uint256(13), uint256(17)))));
    }
}
