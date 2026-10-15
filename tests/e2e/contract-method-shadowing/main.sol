import * from std;
import * from std.dispatch;
import { choose } from lib;
import lib;

function useImported(n: uint256) returns (uint256) {
    return choose(n);
}

contract ScopedCalls {
    // A bounded recursion verifies that an unqualified call uses this method.
    // #[(2) -> 7]
    function choose(n: uint256) public returns (uint256) {
        if (n == 0) {
            return 7;
        } else {
            return choose(n - 1);
        }
    }

    // The imported function remains available through its module qualifier.
    // #[(2) -> 99]
    function qualified(n: uint256) public returns (uint256) {
        return lib.choose(n);
    }

    // Calls outside the contract use the imported function.
    // #[(2) -> 99]
    function outside(n: uint256) public returns (uint256) {
        return useImported(n);
    }
}
