pragma solcore;

// The length of `new T[](n)` is checked as a `uint256` like any expression,
// so names in it resolve and an integer literal or `let`-inferred integer in
// a length or index position becomes a `uint256` (#162, #163).
import * from std;
import * from std.dispatch;

function three() returns (uint256) {
    return 3;
}

// A closure that allocates with a captured length.
function sized(n: uint256) returns (function () returns (uint256)) {
    return lam () -> uint256 {
        return new uint256[](n).length;
    };
}

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

contract ArrayLengthIndex {
    count: uint256;
    xs: uint256[];

    // #[() -> 2]
    function castLength() public returns (uint256) {
        return new uint256[](uint256(2)).length;
    }

    // #[() -> 3]
    function letLength() public returns (uint256) {
        let n = 3;
        let m: uint256[] = new uint256[](n);
        return m.length;
    }

    // #[() -> 7]
    function sliceIndex() public returns (uint256) {
        let a: uint256[] = new uint256[](3);
        a[1] = uint256(7);
        let s = a[1:];
        return uint256(Typedef.rep(s[0]));
    }

    // #[() -> 5]
    function arithmeticLength() public returns (uint256) {
        return new uint256[](0x2 + 3).length;
    }

    // #[() -> 3]
    function callLength() public returns (uint256) {
        return new uint256[](three()).length;
    }

    // #[(4) -> 4]
    function fieldLength(n: uint256) public returns (uint256) {
        count = n;
        return new uint256[](count).length;
    }

    // #[(6) -> 6]
    function convertedLength(w: uint256) public returns (uint256) {
        let n: word = Typedef.rep(w);
        return new uint256[](Typedef.abs(n)).length;
    }

    // #[(5) -> 5]
    function capturedLength(n: uint256) public returns (uint256) {
        let f = sized(n);
        return f();
    }

    // #[() -> 4]
    function lengthOfLength() public returns (uint256) {
        return new uint256[](new uint256[](3).length + 1).length;
    }

    // #[() -> 2]
    function nestedLength() public returns (uint256) {
        let n = 2;
        let m: uint256[][] = new uint256[][](n);
        return m.length + m[1].length;
    }

    // #[() -> 9]
    function letSliceIndex() public returns (uint256) {
        let a: uint256[] = new uint256[](4);
        a[3] = uint256(9);
        let s = a[1:];
        let i = 2;
        return uint256(Typedef.rep(s[i]));
    }

    // #[() -> 8]
    function resliceIndex() public returns (uint256) {
        let a: uint256[] = new uint256[](4);
        a[3] = uint256(8);
        return uint256(Typedef.rep(a[1:][1:][1]));
    }

    // #[() -> 6]
    function storageSliceIndex() public returns (uint256) {
        xs.setLength(uint256(0));
        xs.push(uint256(5));
        xs.push(uint256(6));
        return uint256(Typedef.rep(xs[1:][0]));
    }

    // #[() -> 6]
    function storageIndex() public returns (uint256) {
        xs.setLength(uint256(0));
        xs.push(uint256(5));
        xs.push(uint256(6));
        return uint256(Typedef.rep(xs[1]));
    }

    // #[() -> 7]
    function nestedIndex() public returns (uint256) {
        let g: uint256[][] = new uint256[][](2);
        g[0] = new uint256[](3);
        g[0][1] = uint256(7);
        return uint256(Typedef.rep(g[0][1]));
    }

    // #[() -> 9]
    function userIndex() public returns (uint256) {
        let q = Squares(uint256(10));
        return uint256(Typedef.rep(q[3]));
    }

    // #[() -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000032)]
    function sliceIndexOutOfBounds() public returns (uint256) {
        let a: uint256[] = new uint256[](2);
        let s = a[1:];
        return uint256(Typedef.rep(s[1]));
    }
}
