pragma solcore;

import * from std;
import * from std.dispatch;

enum Wide {
    Wide(word)
}

impl StorageSize<Wide> {
    function size(x: Proxy<Wide>) returns (word) {
        let result: word;
        assembly {
            result := sload(99)
        }
        return result;
    }
}

impl CanStore<storage<Wide>, Wide> {
    function store(r: storage<Wide>, v: Wide) returns () {
        let slot: word;
        let value: word;
        match (r) {
            case storage(x) {
                slot = x;
            }
        }
        match (v) {
            case Wide(x) {
                value = x;
            }
        }
        assembly {
            sstore(slot, value)
        }
    }
    function load(r: storage<Wide>) returns (Wide) {
        let slot: word;
        let value: word;
        match (r) {
            case storage(x) {
                slot = x;
            }
        }
        assembly {
            value := sload(slot)
        }
        return Wide(value);
    }
}

contract C {
    prefix: string;
    first: Wide;
    second: uint256;

    function bump(v: uint256) public returns (uint256) {
        second += v;
        return v;
    }
}
