pragma solcore;

// A leading-dot constructor assigned to a storage location names a
// constructor of the declared type of that location (#174).
import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.StorageGeneric;

enum U {
    A,
    B
}

function tag(u: U) returns (uint256) {
    match (u) {
        case U.A {
            return 1;
        }
        case U.B {
            return 2;
        }
    }
}

contract DotCtorStorageAssign {
    stored: U;
    table: mapping(uint256 => U);

    // #[() -> 2]
    function field() public returns (uint256) {
        stored = .B;
        return tag(stored);
    }

    // #[(true) -> 1]
    // #[(false) -> 2]
    function conditional(c: bool) public returns (uint256) {
        stored = c ? .A : .B;
        return tag(stored);
    }

    // #[() -> (2, 1)]
    function entry() public returns (uint256, uint256) {
        table[1] = .B;
        table[2] = .A;
        return (tag(table[1]), tag(table[2]));
    }
}
