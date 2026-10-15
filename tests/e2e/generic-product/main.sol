import * from std;
import * from std.dispatch;
import {mload, mstore} from std.opcodes;
import * from std.Generic;
import * from std.ABIGeneric;

pragma no-generic-instance-for Point;

enum Point { Point(uint256, uint256) }

// Only requirement: a Generic impl using the primitive pair type.
// rep = (uint256, uint256) — primitive Solcore pair
impl Generic<Point, (uint256, uint256)> {
    function from(p: Point) returns (uint256, uint256) {
        match (p) {
case Point(x, y) {
return (x, y);
}
}
    }
    function to(t: (uint256, uint256)) returns (Point) {
        match (t) {
case (x, y) {
return Point(x, y);
}
}
    }
}

// #[scenario("generic_product")]
// #[deploy(0x) -> success]
// #[raw(0x0dbc6e9b00000000000000000000000000000000000000000000000000000000000000030000000000000000000000000000000000000000000000000000000000000007, label = "encodeX(3, 7) -> 3") -> success(0x0000000000000000000000000000000000000000000000000000000000000003)]
// #[raw(0xe9c28f9600000000000000000000000000000000000000000000000000000000000000030000000000000000000000000000000000000000000000000000000000000007, label = "encodeY(3, 7) -> 7") -> success(0x0000000000000000000000000000000000000000000000000000000000000007)]
// #[raw(0x0e3fc16f000000000000000000000000000000000000000000000000000000000000000a0000000000000000000000000000000000000000000000000000000000000014, label = "decodeX(10, 20) -> 10") -> success(0x000000000000000000000000000000000000000000000000000000000000000a)]
contract GenericProduct {
    constructor() {}

    // Calls encode; returns word at offset 0 (the x field).
    function encodeX(a: uint256, b: uint256) public returns (uint256) {
        let p : Point = Point(a, b);
        let buf = allocate_zeroed_memory(64);
        encode(p, buf, 0, 64);
        return Typedef.abs(mload(buf));
    }

    // Calls encode; returns word at offset 32 (the y field).
    function encodeY(a: uint256, b: uint256) public returns (uint256) {
        let p : Point = Point(a, b);
        let buf = allocate_zeroed_memory(64);
        encode(p, buf, 0, 64);
        return Typedef.abs(mload(buf + 32));
    }

    // Writes [a][b] into memory, calls decode, returns the x field.
    function decodeX(a: uint256, b: uint256) public returns (uint256) {
        let buf = allocate_zeroed_memory(64);
        mstore(buf, Typedef.rep(a));
        mstore(buf + 32, Typedef.rep(b));
        let rdr : MemoryWordReader = MemoryWordReader(buf);
        let dec : ABIDecoder<Point, MemoryWordReader> = ABIDecoder(rdr);
        let p : Point = decode(dec, 0);
        match (p) {
case Point(x, _) {
return x;
}
}
    }
}
