pragma solcore;

// `c[i]` on a user-defined type reads through its `RValueIdxAccess`
// instance, the same class the std arrays implement.
import * from std;
import * from std.dispatch;

// The squares 0, 1, 4, ... below a bound, computed on demand.
enum Squares {
    Squares(uint256)
}

impl<i> RValueIdxAccess<(Squares, i), uint256> where i: Typedef<word> {
    function lookup(xi: (Squares, i)) returns (uint256) {
        match (xi) {
            case (s, j) {
                let k: uint256 = uint256(Typedef.rep(j));
                match (s) {
                    case Squares(bound) {
                        if (k >= bound) {
                            out_of_bounds();
                        }
                    }
                }
                return k * k;
            }
        }
    }
}

contract UserCollectionIndex {
    // #[(10, 3) -> 9]
    // #[(10, 10) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000032)]
    function square(bound: uint256, i: uint256) public returns (uint256) {
        let squares = Squares(bound);
        return squares[i];
    }
}
