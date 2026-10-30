pragma solcore;

import std;
import { memory, storage, DynArray, array, string, Typedef, CanStore } from std;

function inMemory() returns (memory<DynArray<memory<string>>>) {
    return ["hello"];
}

function explicitConversion() returns (memory<DynArray<memory<string>>>) {
    return [Str.fromString("hello")];
}

function concatenated() returns (memory<DynArray<memory<string>>>) {
    return [std.concatLit("hel", "lo")];
}

function conditional(flag: bool) returns (memory<DynArray<memory<string>>>) {
    return [((flag) ? "yes" : "no")];
}

contract C {
    names: array<string>;

    function setNames() returns () {
        names = ["alice", "bob"];
        return ();
    }

    function clearNames() returns () {
        names = [];
        return ();
    }
}
