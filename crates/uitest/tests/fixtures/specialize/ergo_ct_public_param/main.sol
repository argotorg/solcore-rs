pragma solcore;

// comptime parameter on a *public* contract entry point.  Public entry
// arguments come from calldata at runtime, so this can never be satisfied.
// Should be rejected with a clear "public functions cannot take comptime
// parameters" style error.
import * from std;
import * from std.dispatch;

contract CtPublicParam {
    function double(comptime x: word) public returns (word) {
        return x + x;
    }
}
