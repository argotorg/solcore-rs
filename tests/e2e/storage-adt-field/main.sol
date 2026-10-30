pragma solcore;

import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.StorageGeneric;

// Algebraic data types used directly as contract storage fields, including a
// nested ADT (Option(Triple)).
//
//  - someValue  : Option(uint256)  (sum,            rep sum((), uint256)        -> 2 slots)
//  - triple     : Triple           (product,        rep (uint256,(uint256,uint256)) -> 3 slots)
//  - someTriple : Option(Triple)   (sum of product, rep sum((), Triple)         -> 4 slots)

enum Option<a> {
    None,
    Some(a)
}
enum Triple {
    Triple(uint256, uint256, uint256)
}

// #[scenario("storage_adt_field")]
// constructor() (asserts: Option(uint256)=2, Triple=3, Option(Triple)=4)
// #[deploy(0x) -> success]
// #[raw(0x31f68a08, label = "isSome() -> false (zero-init)") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x20965255, label = "getValue() reverts on None") -> revert(0x)]
// #[raw(0x55241077000000000000000000000000000000000000000000000000000000000000002a, label = "setValue(42)") -> success(0x)]
// #[raw(0x20965255, label = "getValue() -> 42") -> success(0x000000000000000000000000000000000000000000000000000000000000002a)]
// #[raw(0x31f68a08, label = "isSome() -> true") -> success(0x0000000000000000000000000000000000000000000000000000000000000001)]
// #[raw(0x52786e06000000000000000000000000000000000000000000000000000000000000000a0000000000000000000000000000000000000000000000000000000000000014000000000000000000000000000000000000000000000000000000000000001e, label = "setTriple(10,20,30)") -> success(0x)]
// #[raw(0x7ba60a20, label = "tripleSum() -> 60") -> success(0x000000000000000000000000000000000000000000000000000000000000003c)]
// #[raw(0x9b5e9a9d, label = "hasSomeTriple() -> false (zero-init)") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x9650c3bd, label = "someTripleSum() reverts on None") -> revert(0x)]
// #[raw(0x421dfc02000000000000000000000000000000000000000000000000000000000000000700000000000000000000000000000000000000000000000000000000000000080000000000000000000000000000000000000000000000000000000000000009, label = "setSomeTriple(7,8,9)") -> success(0x)]
// #[raw(0x9650c3bd, label = "someTripleSum() -> 24") -> success(0x0000000000000000000000000000000000000000000000000000000000000018)]
// #[raw(0x9b5e9a9d, label = "hasSomeTriple() -> true") -> success(0x0000000000000000000000000000000000000000000000000000000000000001)]
// #[raw(0x20965255, label = "getValue() -> 42 (unaffected by nested writes)") -> success(0x000000000000000000000000000000000000000000000000000000000000002a)]
// #[raw(0x7ba60a20, label = "tripleSum() -> 60 (unaffected)") -> success(0x000000000000000000000000000000000000000000000000000000000000003c)]
// #[raw(0x0f0b502e, label = "clearSomeTriple()") -> success(0x)]
// #[raw(0x9b5e9a9d, label = "hasSomeTriple() -> false after clear") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x9650c3bd, label = "someTripleSum() reverts after clear") -> revert(0x)]
// #[raw(0x7ba60a20, label = "tripleSum() -> 60 (unaffected by clear)") -> success(0x000000000000000000000000000000000000000000000000000000000000003c)]
// #[raw(0x20965255, label = "getValue() -> 42 (unaffected by clear)") -> success(0x000000000000000000000000000000000000000000000000000000000000002a)]
contract C {
    someValue: Option<uint256>;
    triple: Triple;
    someTriple: Option<Triple>;

    constructor() {
        // sum:            1 tag + max(size (), size uint256) = 1 + 1 = 2
        assert(StorageSize.size(@Option<uint256>) == 2);
        // product:        size uint256 * 3                   = 3
        assert(StorageSize.size(@Triple) == 3);
        // sum of product: 1 tag + max(size (), size Triple)  = 1 + 3 = 4
        assert(StorageSize.size(@Option<Triple>) == 4);
    }

    function setValue(v: uint256) public {
        someValue = Option.Some(v);
    }

    function clearValue() public {
        someValue = Option.None;
    }

    function getValue() public returns (uint256) {
        match (someValue) {
            case Option.None {
                revertEmpty();
                return uint256(0);
            }
            case Option.Some(v) {
                return v;
            }
        }
    }

    function isSome() public returns (bool) {
        match (someValue) {
            case Option.None {
                return false;
            }
            case Option.Some(_) {
                return true;
            }
        }
    }

    function setTriple(a: uint256, b: uint256, c: uint256) public {
        triple = Triple(a, b, c);
    }

    function tripleSum() public returns (uint256) {
        match (triple) {
            case Triple(a, b, c) {
                return a + b + c;
            }
        }
    }

    // Nested ADT: Option(Triple).
    function setSomeTriple(a: uint256, b: uint256, c: uint256) public {
        someTriple = Option.Some(Triple(a, b, c));
    }

    function clearSomeTriple() public {
        someTriple = Option.None;
    }

    function someTripleSum() public returns (uint256) {
        match (someTriple) {
            case Option.None {
                revertEmpty();
                return uint256(0);
            }
            case Option.Some(t) {
                match (t) {
                    case Triple(a, b, c) {
                        return a + b + c;
                    }
                }
            }
        }
    }

    function hasSomeTriple() public returns (bool) {
        match (someTriple) {
            case Option.None {
                return false;
            }
            case Option.Some(_) {
                return true;
            }
        }
    }
}
