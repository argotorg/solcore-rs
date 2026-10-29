pragma solcore;

import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.StorageGeneric;

enum Blob {
    NoBlob,
    SomeBytes(memory<bytes>)
}

// #[scenario("storage_dynamic_field")]
// constructor() (asserts StorageSize.size(Blob)=2; sets blob=NoBlob)
// #[deploy(0x) -> success]
// #[raw(0x681fe70c, label = "isEmpty() -> true (constructor set NoBlob)") -> success(0x0000000000000000000000000000000000000000000000000000000000000001)]
// #[raw(0x0bcd3b33, label = "getBytes() reverts on NoBlob") -> revert(0x)]
// #[raw(0xda359dc800000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000004deadbeef00000000000000000000000000000000000000000000000000000000, label = "setBytes(0xdeadbeef) stores SomeBytes via storage(bytes)") -> success(0x)]
// #[raw(0x681fe70c, label = "isEmpty() -> false (tag is SomeBytes)") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x0bcd3b33, label = "getBytes() -> 0xdeadbeef (round-trips the dynamic leaf out of storage)") -> success(0x00000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000004deadbeef00000000000000000000000000000000000000000000000000000000)]
// #[raw(0x52efea6e, label = "clear() resets blob=NoBlob") -> success(0x)]
// #[raw(0x681fe70c, label = "isEmpty() -> true (back to NoBlob after clear)") -> success(0x0000000000000000000000000000000000000000000000000000000000000001)]
// #[raw(0x0bcd3b33, label = "getBytes() reverts on NoBlob after clear") -> revert(0x)]
// #[raw(0xda359dc800000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000000, label = "setBytes(0x) stores SomeBytes with a zero-length dynamic payload") -> success(0x)]
// #[raw(0x681fe70c, label = "isEmpty() -> false (SomeBytes(empty) is distinct from NoBlob)") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x0bcd3b33, label = "getBytes() -> 0x (round-trips an empty dynamic payload)") -> success(0x00000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0xda359dc80000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000002111111111111111111111111111111111111111111111111111111111111111111100000000000000000000000000000000000000000000000000000000000000, label = "setBytes(33 bytes of 0x11) overwrites with a multi-slot dynamic payload") -> success(0x)]
// #[raw(0x681fe70c, label = "isEmpty() -> false (still SomeBytes after overwrite)") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x0bcd3b33, label = "getBytes() -> 33 bytes of 0x11 (multi-slot dynamic payload round-trips)") -> success(0x0000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000002111111111111111111111111111111111111111111111111111111111111111111100000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0xda359dc800000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000004deadbeef00000000000000000000000000000000000000000000000000000000, label = "setBytes(0xdeadbeef) shrinks a long (>31 byte) payload back to a short one") -> success(0x)]
// #[raw(0x0bcd3b33, label = "getBytes() -> 0xdeadbeef (no bytes leak from the previous long payload)") -> success(0x00000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000004deadbeef00000000000000000000000000000000000000000000000000000000)]
// #[raw(0xda359dc800000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000000, label = "setBytes(0x) shrinks a short payload to empty") -> success(0x)]
// #[raw(0x0bcd3b33, label = "getBytes() -> 0x") -> success(0x00000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0xda359dc8000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000202222222222222222222222222222222222222222222222222222222222222222, label = "setBytes(32 bytes of 0x22) sits exactly on the short/long boundary") -> success(0x)]
// #[raw(0x0bcd3b33, label = "getBytes() -> 32 bytes of 0x22") -> success(0x000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000202222222222222222222222222222222222222222222222222222222222222222)]
// #[raw(0x681fe70c, label = "isEmpty() -> false") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
contract C {
    blob: Blob;

    constructor() {
        blob = Blob.NoBlob;
        // A dynamic field occupies one slot, so the sum is 1 (tag) + max(0, 1).
        assert(StorageSize.size(@Blob) == 2);
    }

    function clear() public {
        blob = Blob.NoBlob;
    }

    // Stores the memory(bytes) payload into the ADT field (round-trips the
    // dynamic leaf through storage(bytes)).
    function setBytes(b: memory<bytes>) public {
        blob = Blob.SomeBytes(b);
    }

    function getBytes() public returns (memory<bytes>) {
        match (blob) {
            case Blob.NoBlob {
                revertEmpty();
                return memory(0);
            }
            case Blob.SomeBytes(b) {
                return b;
            }
        }
    }

    // Loads the whole ADT back from storage and inspects its tag.
    function isEmpty() public returns (bool) {
        match (blob) {
            case Blob.NoBlob {
                return true;
            }
            case Blob.SomeBytes(_) {
                return false;
            }
        }
    }
}
