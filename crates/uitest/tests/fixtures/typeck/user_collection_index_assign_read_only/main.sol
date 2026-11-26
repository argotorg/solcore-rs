pragma solcore;

import * from std;

// Read-only: there is no `LValueIdxAccess` instance.
enum Squares {
    Squares(uint256)
}

impl<i> RValueIdxAccess<(Squares, i), uint256> where i: Typedef<word> {
    function lookup(xi: (Squares, i)) returns (uint256) {
        match (xi) {
            case (s, j) {
                let k: uint256 = uint256(Typedef.rep(j));
                return k * k;
            }
        }
    }
}

function main() returns (uint256) {
    let s = Squares(10);
    s[3] = 7;
    s[4] += 1;
    return s[3];
}
