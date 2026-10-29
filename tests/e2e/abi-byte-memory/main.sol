pragma solcore;

//~ specialize: loader std-current
//~ specialize: frontend clean
//~ specialize: entry

import * from std;
import * from std.dispatch;
import { mload, mstore } from std.opcodes;

contract AbiByteMemory {
    // byte is an internal word-backed type, not the public ABI's bytes1 type.
    // Its codec preserves the same representation as the original array path.
    // #[(0) -> 0]
    // #[(255) -> 255]
    function scalar(value: uint256) public returns (uint256) {
        let raw = byte(Typedef.rep(value));
        let encoded = abi_encode(raw);
        assert(MemorySize.len(encoded) == 32);
        assert(mload(MemoryPointer.ptr(encoded)) == Typedef.rep(value));
        let decoded: byte = abi_decode(encoded, @byte, @MemoryWordReader);
        return uint256(Typedef.rep(decoded));
    }

    // #[(17) -> 17]
    function calldataScalar(value: uint256) public returns (uint256) {
        let decoder: ABIDecoder<byte, CalldataWordReader> = ABIDecoder(CalldataWordReader(4));
        let decoded: byte = ABIDecode.decode(decoder, 0);
        assert(Typedef.rep(decoded) == Typedef.rep(value));
        return uint256(Typedef.rep(decoded));
    }

    // Verify the encoded slots independently of the decoder.
    // #[() -> 256]
    function array() public returns (uint256) {
        let values: memory<DynArray<byte>> = [byte(0), byte(1), byte(255)];
        let encoded = abi_encode(values);
        let start = MemoryPointer.ptr(encoded);
        assert(MemorySize.len(encoded) == 160);
        assert(mload(start) == 32);
        assert(mload(start + 32) == 3);
        assert(mload(start + 64) == 0);
        assert(mload(start + 96) == 1);
        assert(mload(start + 128) == 255);
        let decoded: memory<DynArray<byte>> =
        abi_decode(encoded, @memory<DynArray<byte>>, @MemoryWordReader);
        return uint256(Typedef.rep(decoded[0]) + Typedef.rep(decoded[1]) + Typedef.rep(decoded[2]));
    }

    // #[() -> 3]
    function nestedAndEmpty() public returns (uint256) {
        let empty = allocateDynamicArray(@byte, 0);
        let nonempty: memory<DynArray<byte>> = [byte(1), byte(2)];
        let values: memory<DynArray<memory<DynArray<byte>>>> = [empty, nonempty];
        let decoded: memory<DynArray<memory<DynArray<byte>>>> =
        abi_decode(abi_encode(values), @memory<DynArray<memory<DynArray<byte>>>>, @MemoryWordReader);
        assert(mload(Typedef.rep(decoded)) == 2);
        assert(mload(Typedef.rep(decoded[0])) == 0);
        return uint256(Typedef.rep(decoded[1][0]) + Typedef.rep(decoded[1][1]));
    }

    // #[() -> revert(0x08638556)]
    function truncatedScalar() public returns (uint256) {
        let encoded: memory<bytes> = memory(allocate_zeroed_memory(64));
        mstore(Typedef.rep(encoded), 31);
        let decoded: byte = abi_decode(encoded, @byte, @MemoryWordReader);
        return uint256(Typedef.rep(decoded));
    }

    // #[() -> revert(0x08638556)]
    function truncatedCalldata() public returns (uint256) {
        let decoder: ABIDecoder<byte, CalldataWordReader> = ABIDecoder(CalldataWordReader(4));
        let decoded: byte = ABIDecode.decode(decoder, 0);
        return uint256(Typedef.rep(decoded));
    }

    // #[() -> revert(0x08638556)]
    function truncatedArray() public returns (uint256) {
        let values: memory<DynArray<byte>> = [byte(1), byte(2)];
        let encoded = abi_encode(values);
        mstore(Typedef.rep(encoded), 96);
        let decoded: memory<DynArray<byte>> =
        abi_decode(encoded, @memory<DynArray<byte>>, @MemoryWordReader);
        return uint256(Typedef.rep(decoded[0]));
    }
}
