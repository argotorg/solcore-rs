pragma solcore;

// Mirrors historical corpus test/examples/cases/fallback-with-args.sol
// (expected failure there): fallback must take no arguments.
import * from std;
import * from std.dispatch;

contract BadFallback {
    constructor() {}

    fallback(x: uint256) {
        revert("fallback-was-called");
    }
}
