pragma solcore;

import * from std;
import * from std.dispatch;
import { Generic } from std.Generic;

trait Local<t> {
    function value(x: t) returns (word);
}
trait Pick<t, r> {
    function pick(x: t) returns (word);
}
trait Delegate<t> {
    function direct(x: t) returns (word) where t: Local;
    function mixed<u>(x: t, y: u) returns (word) where t: Local, u: Local;
    function nested<u, r>(x: t, y: u) returns (word) where(t, u): Pick<(t, r)>;
}

impl Local<word> {
    function value(x: word) returns (word) {
        return x;
    }
}
impl Pick<(word, word), (word, bool)> {
    function pick(x: (word, word)) returns (word) {
        match (x) {
            case ((first, second)) {
                return second;
            }
        }
    }
}
impl Delegate<word> {
    function direct(x: word) returns (word) where word: Local {
        return Local.value(x);
    }
    function mixed<u>(x: word, y: u) returns (word) where word: Local, u: Local {
        return primAddWord(Local.value(x), Local.value(y));
    }
    function nested<u, r>(x: word, y: u) returns (word) where(word, u): Pick<(word, r)> {
        return Pick.pick((x, y));
    }
}

#[derive(Delegate)]
enum Wrapped {
    Wrapped(word)
}
impl Local<Wrapped> {
    function value(x: Wrapped) returns (word) {
        return 99;
    }
}
impl Pick<(Wrapped, word), (Wrapped, bool)> {
    function pick(x: (Wrapped, word)) returns (word) {
        return 999;
    }
}
function apply(f: function (Wrapped) returns (word), x: Wrapped) returns (word) {
    return f(x);
}

contract DerivedMethodConstraints {
    // #[(7) -> 7]
    // #[(42) -> 42]
    function direct(x: uint256) public returns (uint256) {
        return uint256(Delegate.direct(Wrapped(Typedef.rep(x))));
    }

    // #[(7) -> 7]
    function value(x: uint256) public returns (uint256) {
        return uint256(apply(Delegate.direct, Wrapped(Typedef.rep(x))));
    }

    // #[(42) -> 42]
    function closure(x: uint256) public returns (uint256) {
        return uint256(apply(
                lam (w: Wrapped) -> word {
                    return Delegate.direct(w);
                },
                Wrapped(Typedef.rep(x))
            ));
    }

    // #[(7,3) -> 106]
    // #[(42,9) -> 141]
    function mixed(x: uint256, y: uint256) public returns (uint256) {
        return uint256(Delegate.mixed(Wrapped(Typedef.rep(x)), Wrapped(Typedef.rep(y))));
    }

    // #[(7,3) -> 3]
    // #[(42,9) -> 9]
    function nested(x: uint256, y: uint256) public returns (uint256) {
        return uint256(Delegate.nested(Wrapped(Typedef.rep(x)), Typedef.rep(y)));
    }
}
