pragma solcore;

import { storage, array, uint256, Typedef, CanStore } from std;

contract C {
    xs: array<word>;
    n: word;

    function read(i: uint256) returns (word) {
        return xs[i];
    }

    function aliasRead(i: uint256) returns (word) {
        let ys = xs;
        return ys[i];
    }

    function aliasWrite(i: uint256, value: word) returns () {
        let ys = xs;
        ys[i] = value;
        return ();
    }

    function set(x: word) returns () {
        xs = [x, x];
        return ();
    }

    function bad(x: word) returns () {
        n = [x];
        return ();
    }
}
