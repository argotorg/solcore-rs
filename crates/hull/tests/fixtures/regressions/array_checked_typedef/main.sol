pragma solcore;

//~ hull: valid

import * from std;

enum Shifted {
    Shifted(word)
}
impl Typedef<Shifted, word> {
    function rep(x: Shifted) returns (word) {
        match (x) {
            case Shifted(w) {
                return w + 100;
            }
        }
    }
    function abs(w: word) returns (Shifted) {
        return Shifted(w - 100);
    }
}

enum Second {
    Second(word)
}
impl Typedef<Second, word> {
    function rep(x: Second) returns (word) {
        match (x) {
            case Second(w) {
                return w + 1;
            }
        }
    }
    function abs(w: word) returns (Second) {
        return Second(w - 1);
    }
}

type Numbers = array<uint256>;

contract CheckedArrays {
    xs: Numbers;
    seed: word;

    function main() returns (word) {
        let m: memory<DynArray<Shifted>> = [Shifted(3), Shifted(4)];
        xs = [10, 20];
        let p: storage<Numbers> = xs;
        let idx: Second = Second(seed);
        p[idx] += uint256(1);
        let picked: Shifted = m[idx];
        return Typedef.rep(picked) + Typedef.rep(p[idx]);
    }
}
