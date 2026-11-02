pragma solcore;

import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;

export { Fixed, Decoded };

enum Fixed {
    Fixed(bytes32)
}
enum Decoded {
    Decoded(word)
}

// User code cannot implement std's traits for std types (SC0246). std gives
// `bytes32` and `word` these behaviours through default impls, so these impls
// would still be selected, and the external ABI check must find them through
// the derived ABI of the types this module exports.
impl ABIAttribs<bytes32> {
    function headSize(p: Proxy<bytes32>) returns (word) {
        return 64;
    }
    function isStatic(p: Proxy<bytes32>) returns (bool) {
        return false;
    }
}

impl ABIDecode<ABIDecoder<word, MemoryWordReader>, word> {
    function decode(d: ABIDecoder<word, MemoryWordReader>, offset: word) returns (word) {
        return 42;
    }
}
