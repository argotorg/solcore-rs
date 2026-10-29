pragma solcore;

// #[scenario("assembly_string_escapes")]
// #[deploy(0x, value = 0) -> success]
// #[raw(0x, value = 0, label = "suite `assembly_string_escapes` test #2") -> success(0x610a09225c620000000000000000000000000000000000000000000000000000)]
contract AssemblyStringEscapes {
    function main() returns (word) {
        let value: word;
        assembly {
            value := "a\n\t\"\\b"
            switch "\n"
            case "\n" {}
            default {
                revert(0, 0)
            }
            mstore(0, value)
            return (0, 32)
        }
        return value;
    }
}
