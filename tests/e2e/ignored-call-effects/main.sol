function writeSlot() returns (word) {
    assembly { sstore(0, add(sload(0), 1)) }
    return 0;
}
function second(first: word, value: word) returns (word) { return value; }
// #[scenario("ignored_call_effects")]
// #[deploy(0x) -> success]
// #[raw(0x, label = "suite `ignored_call_effects` test #2") -> success(0x0000000000000000000000000000000000000000000000000000000000000047)]
// #[raw(0x, label = "suite `ignored_call_effects` test #3") -> success(0x0000000000000000000000000000000000000000000000000000000000000048)]
contract IgnoredCallEffects {
    function main() returns (word) {
        // Even an unused argument must execute exactly once.
        let result: word = second(writeSlot(), 7);
        assembly {
            mstore(0, add(mul(result, 10), sload(0)))
            return(0, 32)
        }
        return result;
    }
}
