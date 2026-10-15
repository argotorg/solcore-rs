import * from std;
import * from std.dispatch;

function secondUnit(first: (), value: word) returns (word) { return value; }
function secondString(first: memory<string>, value: word) returns (word) { return value; }

function abortWrapped() returns (()) { return revertLit("wrapped"); }

function twiceAddress(value: memory<string>) returns (word) {
    match (value) { case memory(first) {
        match (value) { case memory(second) { return first + second; } }
    } }
}

// #[scenario("ignored_builtin_effects")]
// #[deploy(0x) -> success]
// #[raw(0x222b1407, label = "direct(): unused revertLit") -> revert(0x646972656374)]
// #[raw(0x50e70d48, label = "wrapped(): unused revertLit through a helper") -> revert(0x77726170706564)]
// #[raw(0xc195542e, label = "ignoredString(): result 7, allocated 64 bytes") -> success(0x00000000000000000000000000000000000000000000000000000000000000070000000000000000000000000000000000000000000000000000000000000040)]
// #[raw(0x96418f1c, label = "reusedString(): both addresses equal allocation start, offset 7, allocated 64 bytes") -> success(0x00000000000000000000000000000000000000000000000000000000000000070000000000000000000000000000000000000000000000000000000000000040)]
contract IgnoredBuiltinEffects {
    function direct() public returns (uint256) {
        // Ignoring the unit result must not suppress a direct revert.
        return uint256(secondUnit(revertLit("direct"), 7));
    }

    function wrapped() public returns (uint256) {
        // The same effect must propagate through a user function.
        return uint256(secondUnit(abortWrapped(), 7));
    }

    function ignoredString() public returns (uint256, uint256) {
        let before: word;
        assembly { before := mload(0x40) }
        // Even unused materialization must allocate exactly once.
        let result: word = secondString(Str.fromString("allocated"), 7);
        let after: word;
        assembly { after := mload(0x40) }
        return (uint256(result), uint256(after - before));
    }

    function reusedString() public returns (uint256, uint256) {
        let before: word;
        assembly { before := mload(0x40) }
        // Both parameter uses refer to one allocation. Keep a known
        // numeric binding live across the memory effect as well.
        let offset: word = 7;
        let result: word = twiceAddress(Str.fromString("allocated")) + offset;
        let after: word;
        assembly { after := mload(0x40) }
        return (uint256(result - 2 * before), uint256(after - before));
    }
}
