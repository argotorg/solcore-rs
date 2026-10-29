pragma solcore;

import * from std;
import * from std.signed;
import * from std.unsigned;

type SignedAlias = int8;

contract IntEcho {
    function echo(
        a: SignedAlias,
        b: int16,
        c: int32,
        d: int64,
        e: int128,
        f: int256,
        g: uint8,
        h: uint16,
        i: uint32,
        j: uint64,
        k: uint128
    ) public returns (int8, int16, int32, int64, int128, int256, uint8, uint16, uint32, uint64, uint128) {
        return (a, b, c, d, e, f, g, h, i, j, k);
    }
}
