pragma solcore;

//~ specialize: loader std-current
//~ specialize: frontend clean
//~ specialize: entry

import * from std;
import * from std.dispatch;
import { mload, mstore } from std.opcodes;

contract AbiWordMemory {
    // Raw words use the full 256-bit slot internally, without changing the
    // public ABI's supported type names.
    // #[(0) -> 0]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
    function scalar(value: uint256) public returns (uint256) {
        let raw: word = Typedef.rep(value);
        let encoded = abi_encode(raw);
        assert(MemorySize.len(encoded) == 32);
        assert(mload(MemoryPointer.ptr(encoded)) == raw);
        let decoded: word = abi_decode(encoded, @word, @MemoryWordReader);
        return uint256(decoded);
    }

    // #[(17) -> 17]
    function calldataScalar(value: uint256) public returns (uint256) {
        let decoder: ABIDecoder<word, CalldataWordReader> = ABIDecoder(CalldataWordReader(4));
        let decoded: word = ABIDecode.decode(decoder, 0);
        assert(decoded == Typedef.rep(value));
        return uint256(decoded);
    }

    // Check the wire independently of the decoder, including boundary values.
    // #[(42) -> 42]
    function array(value: uint256) public returns (uint256) {
        let values: memory<DynArray<word>> = [0, Typedef.rep(value), ~0];
        let encoded = abi_encode(values);
        let start = MemoryPointer.ptr(encoded);
        assert(MemorySize.len(encoded) == 160);
        assert(mload(start) == 32);
        assert(mload(start + 32) == 3);
        assert(mload(start + 64) == 0);
        assert(mload(start + 96) == Typedef.rep(value));
        assert(mload(start + 128) == ~0);
        let decoded: memory<DynArray<word>> =
        abi_decode(encoded, @memory<DynArray<word>>, @MemoryWordReader);
        assert(mload(Typedef.rep(decoded)) == 3);
        assert(decoded[0] == 0);
        assert(decoded[2] == ~0);
        return uint256(decoded[1]);
    }

    // #[() -> 16]
    function tuple() public returns (uint256) {
        let values: memory<DynArray<word>> = [7, 9];
        let encoded = abi_encode((1, values));
        let decoded: (word, memory<DynArray<word>>) =
        abi_decode(encoded, @(word, memory<DynArray<word>>), @MemoryWordReader);
        match (decoded) {
            case (first, items) {
                assert(first == 1);
                return uint256(items[0] + items[1]);
            }
        }
    }

    // #[() -> 3]
    function nestedAndEmpty() public returns (uint256) {
        let empty = allocateDynamicArray(@word, 0);
        let nonempty: memory<DynArray<word>> = [1, 2];
        let values: memory<DynArray<memory<DynArray<word>>>> = [empty, nonempty];
        let decoded: memory<DynArray<memory<DynArray<word>>>> =
        abi_decode(abi_encode(values), @memory<DynArray<memory<DynArray<word>>>>, @MemoryWordReader);
        assert(mload(Typedef.rep(decoded)) == 2);
        assert(mload(Typedef.rep(decoded[0])) == 0);
        return uint256(decoded[1][0] + decoded[1][1]);
    }

    // The raw-word decoder must retain the reader's bounds checks.
    // #[() -> revert(0x08638556)]
    function truncatedScalar() public returns (uint256) {
        let encoded: memory<bytes> = memory(allocate_zeroed_memory(64));
        mstore(Typedef.rep(encoded), 31);
        let decoded: word = abi_decode(encoded, @word, @MemoryWordReader);
        return uint256(decoded);
    }

    // #[() -> revert(0x08638556)]
    function truncatedCalldata() public returns (uint256) {
        let decoder: ABIDecoder<word, CalldataWordReader> = ABIDecoder(CalldataWordReader(4));
        let decoded: word = ABIDecode.decode(decoder, 0);
        return uint256(decoded);
    }

    // #[() -> revert(0x08638556)]
    function truncatedArray() public returns (uint256) {
        let values: memory<DynArray<word>> = [7, 9];
        let encoded = abi_encode(values);
        mstore(Typedef.rep(encoded), 96);
        let decoded: memory<DynArray<word>> =
        abi_decode(encoded, @memory<DynArray<word>>, @MemoryWordReader);
        return uint256(decoded[0]);
    }
}
