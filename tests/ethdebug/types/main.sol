pragma solcore;
import * from std;
import * from std.dispatch;
import { uint8 } from std.unsigned;

// Data types without a storage instance: the test describes them as types.
enum Shape {
    Dot(uint256),
    Seg(uint256, uint256),
    Nothing
}

enum Pair<a, b> {
    Pair(a, b)
}

enum List<a> {
    Nil,
    Cons(a, List<a>)
}

enum Never {}

struct Point {
    x: word;
    y: word;
}

contract Types {
    small: uint8;
    flag: bool;
    owner: address;
    hash: bytes32;
    text: memory<string>;
    nested: mapping(address => mapping(word => bool));
    words: word[];

    function main() public returns (uint256) {
        return uint256(0);
    }
}

// Where storage fields are known to be: after fields of known size only.
contract Early {
    a: uint256;
    b: string;
    c: bytes32;
    d: memory<string>;
    e: mapping(uint256 => address);
    f: mapping(address => mapping(word => bool));
    g: mapping(string => uint256);
    h: address;

    function main() public returns (uint256) {
        a = uint256(1);
        c = bytes32(3);
        h = address(8);
        return uint256(0);
    }
}

// A bool has no explicit size in std, so no later field has a known slot.
contract AfterBool {
    a: word;
    b: bool;
    c: uint256;
    d: mapping(word => word);

    function main() public returns (uint256) {
        a = 1;
        c = uint256(3);
        return uint256(0);
    }
}

// A `()` field is one slot in Hull and none in std, so no later field has a
// known slot.
contract AfterUnit {
    v: word;
    u: ();
    w: word;

    function main() public returns (uint256) {
        v = 3;
        w = 7;
        return uint256(0);
    }
}
