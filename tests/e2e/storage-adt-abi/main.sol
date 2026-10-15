import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;
import * from std.StorageGeneric;

// An ADT crossing the ABI boundary AND living in storage at the same time.
//
// The public entry points take and return `Option(uint256)` directly, so the
// generated dispatch has to
//   * derive a variant-aware ABIDecode + ABIEncode for the type
//     (DeriveGeneric.buildABIDecodeSum / buildABIEncode),
//   * reach ABIAttribs through the concrete derived instance, and
//   * build the *method* selector from the derived SigString, which for a sum is
//     still the structural string "sum(<l>,<r>)" — here rep = sum((), uint256) and
//     sigStr(()) = "", so the signature is `setOpt(sum(,uint256))`. (The method
//     selector is unrelated to the per-variant value tag below.)
//
// Wire layout of a sum (std.ABIGeneric): one bytes32 variant tag, then the branch
// payload at +32. The tag is keccak256 of the variant signature `Name(argSigs)`:
//   Some(42) -> keccak256("Some(uint256)") followed by 0x...2a
//   None     -> keccak256("None()")        followed by a don't-care word
// (This replaced the old positional 0x..00/0x..01 inl/inr tag.)

enum Option<a> { None, Some(a) }

// #[scenario("storage_adt_abi")]
// constructor() (stored = None)
// #[deploy(0x) -> success]
// #[raw(0x31f68a08, label = "isSome() -> false") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x9e0708f29c7ca87705d5f9eeaab8a20e33ae9cb37db0cac83fe4b24443db4b69535afde9000000000000000000000000000000000000000000000000000000000000002a, label = "setOpt(Some(42)) : keccak256(\"Some(uint256)\") tag then payload 42") -> success(0x)]
// #[raw(0x31f68a08, label = "isSome() -> true") -> success(0x0000000000000000000000000000000000000000000000000000000000000001)]
// #[raw(0x80a275860000000000000000000000000000000000000000000000000000000000000007, label = "unwrapOr(7) -> 42") -> success(0x000000000000000000000000000000000000000000000000000000000000002a)]
// #[raw(0xdfbe8f99, label = "getOpt() -> Some(42) encoded as keccak256(\"Some(uint256)\") tag + payload 42") -> success(0x9c7ca87705d5f9eeaab8a20e33ae9cb37db0cac83fe4b24443db4b69535afde9000000000000000000000000000000000000000000000000000000000000002a)]
// #[raw(0xf12ca2db9c7ca87705d5f9eeaab8a20e33ae9cb37db0cac83fe4b24443db4b69535afde90000000000000000000000000000000000000000000000000000000000000009, label = "echo(Some(9)) round-trips without touching storage") -> success(0x9c7ca87705d5f9eeaab8a20e33ae9cb37db0cac83fe4b24443db4b69535afde90000000000000000000000000000000000000000000000000000000000000009)]
// #[raw(0x9e0708f2312343540cf19bad8e24ee12fb32c550e430a21136a53271f61c15371c880b910000000000000000000000000000000000000000000000000000000000000000, label = "setOpt(None) : keccak256(\"None()\") tag") -> success(0x)]
// #[raw(0x31f68a08, label = "isSome() -> false") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x80a275860000000000000000000000000000000000000000000000000000000000000007, label = "unwrapOr(7) -> 7") -> success(0x0000000000000000000000000000000000000000000000000000000000000007)]
// #[raw(0xdfbe8f99, label = "getOpt() -> None") -> success(0x312343540cf19bad8e24ee12fb32c550e430a21136a53271f61c15371c880b910000000000000000000000000000000000000000000000000000000000000000)]
contract C {
    stored : Option<uint256>;

    constructor() {
        stored = Option.None;
        assert(StorageSize.size(@Option<uint256>) == 2);
    }

    // ADT as a parameter: decoded from calldata, then written to storage.
    function setOpt(o : Option<uint256>) public returns (()) {
        stored = o;
    }

    // ADT as a return value: loaded from storage, then encoded into returndata.
    function getOpt() public returns (Option<uint256>) {
        return stored;
    }

    // Round-trip in one call, without touching storage.
    function echo(o : Option<uint256>) public returns (Option<uint256>) {
        return o;
    }

    function isSome() public returns (bool) {
        match (stored ) {
        case Option.None    { return false;
        } case Option.Some(_) { return true;
        } }
    }

    function unwrapOr(d : uint256) public returns (uint256) {
        match (stored ) {
        case Option.None    { return d;
        } case Option.Some(v) { return v;
        } }
    }
}
