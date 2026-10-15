import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;

// ABI-decoding a dynamic array whose element is a sum-typed ADT.
//
// `Operation` has two constructors, so its Generic representation is the
// primitive sum `sum(uint256, uint256)` (Approve = inl, Reject = inr). Each
// wire element is therefore two words — a tag word then the payload — which the
// word-per-slot memory(DynArray(...)) representation cannot hold. The array is
// instead decoded lazily from calldata: the parameter becomes a
// `calldata<array<Operation>>` handle to the length word, and elements are
// decoded on demand. Indexing uses the ordinary `ops[i]` sugar (calldata-array
// RValueIdxAccess) and `ops.length()` uses the Length-trait UFCS — the same
// surface syntax as storage arrays. `ops` is a parameter, so this relies on
// value-receiver UFCS (NameResolution), not just the field-receiver form.
enum Operation {
    Approve(uint256),
    Reject(uint256)
}

contract Batch {
    constructor() {}

    // Number of operations in the array.
    function count(ops: calldata<array<Operation>>) public returns (uint256) {
        return ops.length();
    }

    // Constructor of element i, mapped to a distinct sentinel: 16 for Approve,
    // 32 for Reject. Deliberately not 0/1 — those coincide with the on-wire sum
    // tag (inl=0, inr=1), so non-trivial values prove the match actually
    // discriminates the constructor rather than echoing the raw tag word.
    function tagOf(ops: calldata<array<Operation>>, i: uint256) public returns (uint256) {
        let op: Operation = ops[i];
        match (op) {
            case Operation.Approve(_) {
                return uint256(16);
            }
            case Operation.Reject(_) {
                return uint256(32);
            }
        }
    }

    // Payload (the uint256) of element i, regardless of constructor.
    function amountOf(ops: calldata<array<Operation>>, i: uint256) public returns (uint256) {
        let op: Operation = ops[i];
        match (op) {
            case Operation.Approve(v) {
                return v;
            }
            case Operation.Reject(v) {
                return v;
            }
        }
    }
}
