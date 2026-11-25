pragma solcore;
import * from std;
import * from std.dispatch;

// Scopes that end at a jump: a loop with its variable, a block with its own
// variable, and a lambda whose first use of a captured variable is in code that
// compile-time evaluation removes.
contract Scopes {
    stored: uint256;

    function loop_sum() public returns (uint256) {
        let s: uint256 = uint256(0);
        for (let i: uint256 = uint256(0); i < uint256(10); i = i + uint256(1)) {
            if (i == uint256(5)) {
                break;
            } else {}
            s = s + i;
        }
        let after_loop: uint256 = s + uint256(1);
        return after_loop;
    }

    function block_sum(n: uint256) public returns (uint256) {
        let out: uint256 = n;
        if (n == uint256(1)) {
            let inner: uint256 = n + uint256(2);
            stored = inner;
        }
        let after_block: uint256 = out + stored;
        return after_block;
    }

    function captured() public returns (uint256) {
        let base: uint256 = stored + uint256(10);
        let f = lam (v: uint256) -> uint256 {
            if (false) {
                return base;
            }
            return v + base;
        };
        return f(uint256(1));
    }
}

// A local that has the name of a field hides the field.
contract Hiding {
    count: uint256;
    other: uint256;

    function main() public returns (uint256) {
        count = uint256(1);
        let count: uint256 = other + uint256(2);
        other = count;
        return other;
    }
}
