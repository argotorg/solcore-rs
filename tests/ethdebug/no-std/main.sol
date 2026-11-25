pragma solcore;

// A module that sees none of std's field-access classes: Hull gives each field
// its declaration index as slot, so a `()` field takes a slot here.

enum Shape {
    Dot(word),
    Seg(word, word),
    Nothing
}

enum Flip {
    Left(word),
    Right
}

struct Holder {
    s: Shape;
}

contract AfterUnit {
    v: word;
    u: ();
    w: word;

    function main() returns (word) {
        v = 3;
        w = 7;
        let r: word = w;
        assembly { mstore(0, r) return(0, 32) }
        return r;
    }
}

// Three constructors have no description; two constructors do.
contract Tagged {
    a: word;
    s: Shape;
    h: Holder;
    f: Flip;
    w: word;

    function main() returns (word) {
        a = 5;
        w = 9;
        let r: word = w;
        assembly { mstore(0, r) return(0, 32) }
        return r;
    }
}
