pragma solcore;

import * from std;

enum Custom {
    Custom(word)
}

impl Typedef<Custom, word> {
    function rep(value: Custom) returns (word) {
        match (value) {
            case Custom(raw) {
                return raw;
            }
        }
    }

    function abs(raw: word) returns (Custom) {
        return Custom(raw);
    }
}

// A word representation alone does not establish a type's ABI wire format.
function encode(value: Custom) returns (memory<bytes>) {
    return abi_encode(value);
}

function decode(encoded: memory<bytes>) returns (Custom) {
    return abi_decode(encoded, @Custom, @MemoryWordReader);
}
