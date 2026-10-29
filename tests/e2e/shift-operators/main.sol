pragma solcore;

// `<<` and `>>` follow Solidity: the result has the type of the shifted value,
// the amount may be any unsigned type (here a literal `word`, `uint8` and
// `uint256`), bits shifted past the width are discarded, and shifts never
// revert, inside or outside `unchecked`.
import * from std;
import * from std.unsigned;
import * from std.unchecked.unsigned;
import * from std.dispatch;

contract ShiftOperators {
    // #[(5) -> 40]
    function literalAmount(x: uint256) public returns (uint256) {
        return x << 3;
    }

    // #[(0x100, 4) -> 0x10]
    // #[(0x100, 255) -> 0]
    function narrowAmount(x: uint256, s: uint256) public returns (uint256) {
        let k: uint8 = Typedef.abs(Typedef.rep(s));
        return x >> k;
    }

    // #[(1, 255) -> 0x8000000000000000000000000000000000000000000000000000000000000000]
    // #[(1, 256) -> 0]
    function wordShift(x: uint256, s: uint256) public returns (uint256) {
        let w: word = Typedef.rep(x);
        return uint256(w << Typedef.rep(s));
    }

    // #[(1, 8) -> 128]
    function compound(x: uint256, s: uint256) public returns (uint256) {
        let r: uint256 = x;
        r <<= s;
        r >>= 1;
        return r;
    }

    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0x8000000000000000000000000000000000000000000000000000000000000000]
    function neverReverts(x: uint256) public returns (uint256) {
        return x << 255;
    }

    // #[(3, 1) -> 6]
    function insideUnchecked(x: uint256, s: uint256) public returns (uint256) {
        unchecked {
            return x << s;
        }
    }

    // `a + b << 2 & 0xff` is `((a + b) << 2) & 0xff`.
    // #[(1, 2) -> 12]
    // #[(0x40, 0) -> 0]
    function precedence(a: uint256, b: uint256) public returns (uint256) {
        return a + b << 2 & 0xff;
    }

    // #[(2, 1) -> true]
    // #[(1, 2) -> false]
    function comparisons(a: uint256, b: uint256) public returns (bool) {
        return a > b && a >= b;
    }
}
