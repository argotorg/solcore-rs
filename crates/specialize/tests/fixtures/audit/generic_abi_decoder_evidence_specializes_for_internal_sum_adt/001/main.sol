pragma solcore;

//~ specialize: loader std

import * from std;
import * from std.Generic;
import * from std.ABIGeneric;

enum Choice {
    Left(uint256),
    Right(address)
}

contract C {
    function main() returns (word) {
        let buf = allocate_zeroed_memory(64);
        let rdr: MemoryWordReader = MemoryWordReader(buf, buf + 64);
        let dec: ABIDecoder<Choice, MemoryWordReader> =
        ABIDecoder(rdr);
        let value: Choice = decode(dec, 0);
        match (value) {
            case Choice.Left(x) {
                return Typedef.rep(x);
            }
            case Choice.Right(_) {
                return 0;
            }
        }
    }
}
