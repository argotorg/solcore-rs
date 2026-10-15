// #[scenario("asm_scope")]
// #[deploy(0x) -> success]
// #[raw(0x, label = "suite `asm_scope` test #2") -> success(0x0000000000000000000000000000000000000000000000000000000000000093)]
// #[raw(0x00, label = "suite `asm_scope` test #3") -> success(0x00000000000000000000000000000000000000000000000000000000000000fc)]
contract AssemblyScope {
    function main() returns (word) {
        let seed: comptime<word> = 7;
        let value: word = 0;
        let changed: word = 0;
        assembly {
            value := seed
            { let seed := 9 value := add(value, seed) }
            value := add(value, seed)
            // The initializer reads the outer value once; the loop body reads
            // its counter, and the outer binding survives the loop.
            for { let seed := seed } lt(seed, 9) { seed := add(seed, 1) } {
                value := add(value, seed)
            }
            switch calldatasize()
            case 0 { let seed := 100 value := add(value, seed) }
            default { let seed := 200 value := add(value, seed) }
            // A nested shadow must not conceal this write to the outer value.
            if calldatasize() {
                changed := 5
                { let changed := 9 }
            }
            value := add(value, changed)
            function identity(seed) -> result { result := seed }
            value := add(value, identity(2))
        }
        assembly {
            value := add(value, seed)
            mstore(0, value)
            return(0, 32)
        }
        return value;
    }
}
