pragma solcore;

import { storage, array } from std;

contract C {
    xs: array<word>;

    function set(x: word) returns () {
        xs = [x];
        return ();
    }
}
