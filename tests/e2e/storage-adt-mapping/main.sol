pragma solcore;

import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.StorageGeneric;

// An ADT used as the VALUE of a storage mapping.
//
// This is the path opened by routing mapping reads through CanStore instead of
// StorageType (std.sol: readStorage / ridx / RValueIdxAccess). The write side
// already went through Assign -> CanStore.store.
//
// A multi-slot value in a mapping occupies hash2(slot, key) .. + size(v) - 1,
// exactly as Solidity lays out a struct behind a mapping.

enum Option<a> {
    None,
    Some(a)
}
enum Pair {
    Pair(uint256, uint256)
}

// #[scenario("storage_adt_mapping")]
// constructor() (asserts size Option(uint256)=2, Pair=2, Option(Pair)=3)
// #[deploy(0x) -> success]
// #[raw(0x98fd64770000000000000000000000000000000000000000000000000000000000000001, label = "hasOpt(1) -> false on an untouched key (zero slot reads as tag 0 = None)") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0xb64c0df10000000000000000000000000000000000000000000000000000000000000001, label = "getOpt(1) reverts on None") -> revert(0x)]
// #[raw(0x1d26bdb90000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000002a, label = "putOpt(1, 42)") -> success(0x)]
// #[raw(0x98fd64770000000000000000000000000000000000000000000000000000000000000001, label = "hasOpt(1) -> true") -> success(0x0000000000000000000000000000000000000000000000000000000000000001)]
// #[raw(0xb64c0df10000000000000000000000000000000000000000000000000000000000000001, label = "getOpt(1) -> 42 (2-slot value read back from hash2(slot,key))") -> success(0x000000000000000000000000000000000000000000000000000000000000002a)]
// #[raw(0x98fd64770000000000000000000000000000000000000000000000000000000000000002, label = "hasOpt(2) -> false (neighbouring key untouched)") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x1d26bdb900000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000007, label = "putOpt(2, 7)") -> success(0x)]
// #[raw(0xb64c0df10000000000000000000000000000000000000000000000000000000000000001, label = "getOpt(1) -> 42 (keys do not alias)") -> success(0x000000000000000000000000000000000000000000000000000000000000002a)]
// #[raw(0xb64c0df10000000000000000000000000000000000000000000000000000000000000002, label = "getOpt(2) -> 7") -> success(0x0000000000000000000000000000000000000000000000000000000000000007)]
// #[raw(0x3ac6250a0000000000000000000000000000000000000000000000000000000000000001, label = "clearOpt(1) writes the None tag over Some") -> success(0x)]
// #[raw(0x98fd64770000000000000000000000000000000000000000000000000000000000000001, label = "hasOpt(1) -> false") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0xb64c0df10000000000000000000000000000000000000000000000000000000000000002, label = "getOpt(2) -> 7 (clearing key 1 did not touch key 2)") -> success(0x0000000000000000000000000000000000000000000000000000000000000007)]
// #[raw(0x210385e1000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000030000000000000000000000000000000000000000000000000000000000000004, label = "putPair(1, 3, 4) — same key, different mapping, different base slot") -> success(0x)]
// #[raw(0xaf4869350000000000000000000000000000000000000000000000000000000000000001, label = "pairSum(1) -> 7") -> success(0x0000000000000000000000000000000000000000000000000000000000000007)]
// #[raw(0xb64c0df10000000000000000000000000000000000000000000000000000000000000002, label = "getOpt(2) -> 7 (pairs[1] did not collide with opts[2])") -> success(0x0000000000000000000000000000000000000000000000000000000000000007)]
// #[raw(0xe099d2110000000000000000000000000000000000000000000000000000000000000005000000000000000000000000000000000000000000000000000000000000000a0000000000000000000000000000000000000000000000000000000000000014, label = "putOptPair(5, 10, 20) — 3-slot sum-of-product value") -> success(0x)]
// #[raw(0xae2799af0000000000000000000000000000000000000000000000000000000000000005, label = "optPairSum(5) -> 30") -> success(0x000000000000000000000000000000000000000000000000000000000000001e)]
// #[raw(0xd124ca490000000000000000000000000000000000000000000000000000000000000005, label = "clearOptPair(5)") -> success(0x)]
// #[raw(0xae2799af0000000000000000000000000000000000000000000000000000000000000005, label = "optPairSum(5) reverts on None") -> revert(0x)]
// #[raw(0xaf4869350000000000000000000000000000000000000000000000000000000000000001, label = "pairSum(1) -> 7 (still intact)") -> success(0x0000000000000000000000000000000000000000000000000000000000000007)]
contract C {
    // 2 slots per entry: tag + payload
    opts: mapping(uint256 => Option<uint256>);
    // 2 slots per entry: no tag, two words
    pairs: mapping(uint256 => Pair);
    // 3 slots per entry: tag + max(0, 2)
    optPairs: mapping(uint256 => Option<Pair>);

    constructor() {
        assert(StorageSize.size(@Option<uint256>) == 2);
        assert(StorageSize.size(@Pair) == 2);
        assert(StorageSize.size(@Option<Pair>) == 3);
    }

    function putOpt(k: uint256, v: uint256) public {
        opts[k] = Option.Some(v);
    }

    function clearOpt(k: uint256) public {
        opts[k] = Option.None;
    }

    // Unset keys read back as the zero slot pattern, i.e. tag 0 = None.
    function hasOpt(k: uint256) public returns (bool) {
        match (opts[k]) {
            case Option.None {
                return false;
            }
            case Option.Some(_) {
                return true;
            }
        }
    }

    function getOpt(k: uint256) public returns (uint256) {
        match (opts[k]) {
            case Option.None {
                revertEmpty();
                return uint256(0);
            }
            case Option.Some(v) {
                return v;
            }
        }
    }

    function putPair(k: uint256, a: uint256, b: uint256) public {
        pairs[k] = Pair(a, b);
    }

    function pairSum(k: uint256) public returns (uint256) {
        match (pairs[k]) {
            case Pair(a, b) {
                return a + b;
            }
        }
    }

    function putOptPair(k: uint256, a: uint256, b: uint256) public {
        optPairs[k] = Option.Some(Pair(a, b));
    }

    function clearOptPair(k: uint256) public {
        optPairs[k] = Option.None;
    }

    function optPairSum(k: uint256) public returns (uint256) {
        match (optPairs[k]) {
            case Option.None {
                revertEmpty();
                return uint256(0);
            }
            case Option.Some(p) {
                match (p) {
                    case Pair(a, b) {
                        return a + b;
                    }
                }
            }
        }
    }
}
