pragma solcore;

contract C {
    seed: (word, bool) = ((true) ? (1, true) : (2, false));

    function f(x: word, y: bool, z: word) public returns ((word, bool, word)) {
        let t: (word, bool, word) = (x, y, z);
        let b: bool = true;
        match (b) {
            case true {
                return (x, y, z);
            }
            case false {
                return (z, y, x);
            }
        }
        let w: word = ((y) ? x : z);
        if (y) {
            return (w, y, z);
        } else {
            return (z, y, w);
        }
        match (t) {
            case (a, b, c) {
                return (a, b, c);
            }
        }
    }
}
