pragma solcore;

import * from std;
import * from std.dispatch;
import * from base;
import { allA } from a;
import { allB } from b;

contract TraitMethodGenerics {
    // #[(0,21) -> 27]
    // #[(3,99) -> 27]
    function dictionaries(x: uint256, y: uint256) public returns (uint256) {
        let rawX: word = Typedef.rep(x);
        let rawY: word = Typedef.rep(y);
        return uint256(primAddWord(allA(rawX, rawY), allB(rawX, rawY)));
    }

    // #[(3,21) -> 42]
    // #[(0,7) -> 14]
    function shapes(x: uint256, y: uint256) public returns (uint256) {
        let rawX: word = Typedef.rep(x);
        let rawY: word = Typedef.rep(y);
        let scalar: word = Parent.identity(rawX, rawY);
        let pair: (word, word) = Parent.identity(rawX, (rawX, rawY));
        match (pair) {
            case ((p, q)) {
                return uint256(primAddWord(scalar, q));
            }
        }
    }

    // #[(3,21) -> 42]
    function values(x: uint256, y: uint256) public returns (uint256) {
        let rawX: word = Typedef.rep(x);
        let rawY: word = Typedef.rep(y);
        let scalarFn = Parent.identity;
        let scalar: word = scalarFn(rawX, rawY);
        let pairFn = Parent.identity;
        let pair: (word, word) = pairFn(rawX, (rawX, rawY));
        match (pair) {
            case ((p, q)) {
                return uint256(primAddWord(scalar, q));
            }
        }
    }

    // #[(3,21) -> 42]
    function derivedShapes(x: uint256, y: uint256) public returns (uint256) {
        let rawX: word = Typedef.rep(x);
        let rawY: word = Typedef.rep(y);
        let scalar: word = Parent.identity(Derived(rawX), rawY);
        let pair: (word, word) = Parent.identity(Derived(rawX), (rawX, rawY));
        match (pair) {
            case ((p, q)) {
                return uint256(primAddWord(scalar, q));
            }
        }
    }
}
