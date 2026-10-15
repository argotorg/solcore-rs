contract C {
    function main() returns (word) {
        let seed: comptime<word> = 7;
        let value: word = 0;
        assembly {
            value := seed
            {
                let seed := 9
                value := add(value, seed)
            }
            value := add(value, seed)
        }
        // The inner let must also preserve seed after leaving the assembly.
        assembly {
            value := add(value, seed)
        }
        return value;
    }
}
