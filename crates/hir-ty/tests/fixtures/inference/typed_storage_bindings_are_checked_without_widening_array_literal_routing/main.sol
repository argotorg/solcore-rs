pragma solcore;

import * from std;

contract C {
    xs: array<uint256>;

    function good(i: uint256, value: uint256) returns () {
        let typed: storage<array<uint256>> = xs;
        typed[i] = value;
        return ();
    }

    function bad(i: uint256) returns () {
        let typed: storage<array<address>> = xs;
        typed[i] = uint256(1);
        return ();
    }

    function typedLiteral() returns () {
        let values: memory<DynArray<uint256>> = [uint256(1)];
        xs = values;
        return ();
    }
}
