pragma solcore;

import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.StorageGeneric;

// `bool` in storage, bare and inside an ADT.
//
// bool is a builtin rather than a Typedef(word), so it has no StorageType
// instance. It is storable only through the dedicated
// `storage(bool):CanStore(bool)` instance, which round-trips it via
// frombool / tobool. This test pins that instance down, both as a plain
// contract field and as a leaf reached through the structural CanStore
// decomposition of an ADT.
//
// Note the entry points take uint256 rather than bool: bool has no ABIDecode
// instance, so it cannot appear in a public parameter position. It can appear
// in a return position, which is what the getters below exercise.

enum Flags {
    Flags(bool, bool)
}
enum Toggle {
    Off,
    On(bool)
}

function toBool(v: uint256) returns (bool) {
    return v != uint256(0);
}

// #[scenario("storage_adt_bool")]
// constructor() (asserts size(bool)=1, size(Flags)=2, size(Toggle)=2)
// #[deploy(0x) -> success]
// #[raw(0x33ef2640, label = "getBare() -> false (set by the constructor)") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x6de65b180000000000000000000000000000000000000000000000000000000000000001, label = "setBare(1) stores true through storage(bool):CanStore(bool)") -> success(0x)]
// #[raw(0x33ef2640, label = "getBare() -> true") -> success(0x0000000000000000000000000000000000000000000000000000000000000001)]
// #[raw(0x6de65b180000000000000000000000000000000000000000000000000000000000000000, label = "setBare(0) stores false back") -> success(0x)]
// #[raw(0x33ef2640, label = "getBare() -> false") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0xe9a37061, label = "firstFlag() -> false (constructor)") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x459a2790, label = "secondFlag() -> false") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0xc469153a00000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000000, label = "setFlags(1, 0): bool leaves inside a product") -> success(0x)]
// #[raw(0xe9a37061, label = "firstFlag() -> true") -> success(0x0000000000000000000000000000000000000000000000000000000000000001)]
// #[raw(0x459a2790, label = "secondFlag() -> false (the two bool slots are independent)") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0xc469153a00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001, label = "setFlags(0, 1)") -> success(0x)]
// #[raw(0xe9a37061, label = "firstFlag() -> false") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x459a2790, label = "secondFlag() -> true") -> success(0x0000000000000000000000000000000000000000000000000000000000000001)]
// #[raw(0x175bbecf, label = "isOn() -> false (Off, from the constructor)") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x1f6f59be, label = "toggleValue() reverts on Off") -> revert(0x)]
// #[raw(0x681580480000000000000000000000000000000000000000000000000000000000000000, label = "turnOn(0) stores On(false): a bool leaf under a sum tag") -> success(0x)]
// #[raw(0x175bbecf, label = "isOn() -> true (the tag distinguishes On(false) from Off)") -> success(0x0000000000000000000000000000000000000000000000000000000000000001)]
// #[raw(0x1f6f59be, label = "toggleValue() -> false") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x681580480000000000000000000000000000000000000000000000000000000000000001, label = "turnOn(1) stores On(true)") -> success(0x)]
// #[raw(0x175bbecf, label = "isOn() -> true") -> success(0x0000000000000000000000000000000000000000000000000000000000000001)]
// #[raw(0x1f6f59be, label = "toggleValue() -> true") -> success(0x0000000000000000000000000000000000000000000000000000000000000001)]
// #[raw(0xaf10c810, label = "turnOff() writes the Off tag over On(true)") -> success(0x)]
// #[raw(0x175bbecf, label = "isOn() -> false") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x1f6f59be, label = "toggleValue() reverts again (the stale payload slot must not resurrect On)") -> revert(0x)]
// #[raw(0x33ef2640, label = "getBare() -> false (writing `toggle` never disturbed `bare`)") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x459a2790, label = "secondFlag() -> true (writing `toggle` never disturbed `flags`)") -> success(0x0000000000000000000000000000000000000000000000000000000000000001)]
contract C {
    bare: bool;
    flags: Flags;
    toggle: Toggle;

    constructor() {
        bare = false;
        flags = Flags(false, false);
        toggle = Toggle.Off;
        assert(StorageSize.size(@bool) == 1);
        // product of two bools
        assert(StorageSize.size(@Flags) == 2);
        // 1 tag + max(size (), size bool)
        assert(StorageSize.size(@Toggle) == 2);
    }

    function setBare(v: uint256) public {
        bare = toBool(v);
    }

    function getBare() public returns (bool) {
        return bare;
    }

    function setFlags(a: uint256, b: uint256) public {
        flags = Flags(toBool(a), toBool(b));
    }

    function firstFlag() public returns (bool) {
        match (flags) {
            case Flags(a, _) {
                return a;
            }
        }
    }

    function secondFlag() public returns (bool) {
        match (flags) {
            case Flags(_, b) {
                return b;
            }
        }
    }

    function turnOn(v: uint256) public {
        toggle = Toggle.On(toBool(v));
    }

    function turnOff() public {
        toggle = Toggle.Off;
    }

    // Distinguishes Off from On(false): both leave a zero payload slot, so only
    // the tag can tell them apart.
    function isOn() public returns (bool) {
        match (toggle) {
            case Toggle.Off {
                return false;
            }
            case Toggle.On(_) {
                return true;
            }
        }
    }

    function toggleValue() public returns (bool) {
        match (toggle) {
            case Toggle.Off {
                revertEmpty();
                return false;
            }
            case Toggle.On(b) {
                return b;
            }
        }
    }
}
