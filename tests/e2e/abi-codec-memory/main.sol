pragma solcore;

import * from std;
import * from std.dispatch;
import { mload } from std.opcodes;

contract AbiCodecMemory {
    // #[(7) -> 7]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
    function scalar(value: uint256) public returns (uint256) {
        let encoded = abi_encode(value);
        return abi_decode(encoded, @uint256, @MemoryWordReader);
    }

    // The tuple has a nonzero head offset and a dynamic field. Check its wire
    // layout independently of the decoder, then decode the same nested value.
    // #[() -> 28]
    function tuple() public returns (uint256) {
        let text: memory<string> = "a";
        let encoded = abi_encode((uint256(7), ABITuple((uint256(9), text)), uint256(11)));
        let start = MemoryPointer.ptr(encoded);
        assert(MemorySize.len(encoded) == 224);
        assert(mload(start) == 7);
        assert(mload(start + 32) == 96);
        assert(mload(start + 64) == 11);
        assert(mload(start + 96) == 9);
        assert(mload(start + 128) == 64);
        assert(mload(start + 160) == 1);
        let decoded: (uint256, (uint256, memory<string>), uint256) =
        abi_decode(encoded, @(uint256, ABITuple<(uint256, memory<string>)>, uint256), @MemoryWordReader);
        match (decoded) {
            case (first, (inner, value), last) {
                return first + inner + last + uint256(strlen(value));
            }
        }
    }

    // #[() -> 27]
    function staticTuple() public returns (uint256) {
        let encoded = abi_encode((uint256(7), ABITuple((uint256(9), uint256(11)))));
        let decoded: (uint256, (uint256, uint256)) =
        abi_decode(encoded, @(uint256, ABITuple<(uint256, uint256)>), @MemoryWordReader);
        match (decoded) {
            case (first, (a, b)) {
                return first + a + b;
            }
        }
    }

    // #[() -> 27]
    function array() public returns (uint256) {
        let values: memory<DynArray<uint256>> = [uint256(7), uint256(9)];
        let encoded = abi_encode((uint256(11), values));
        let decoded: (uint256, memory<DynArray<uint256>>) =
        abi_decode(encoded, @(uint256, memory<DynArray<uint256>>), @MemoryWordReader);
        match (decoded) {
            case (first, items) {
                assert(mload(Typedef.rep(items)) == 2);
                return first + items[0] + items[1];
            }
        }
    }

    // #[() -> 0]
    function emptyArray() public returns (uint256) {
        let values = allocateDynamicArray(@uint256, 0);
        let decoded: memory<DynArray<uint256>> =
        abi_decode(abi_encode(values), @memory<DynArray<uint256>>, @MemoryWordReader);
        return uint256(mload(Typedef.rep(decoded)));
    }

    // An array's in-memory words are not necessarily its ABI words.
    // #[() -> 305419896]
    function fixedBytesArray() public returns (uint256) {
        let values: memory<DynArray<bytes4>> = [bytes4(0x12345678), bytes4(0xabcdef01)];
        let encoded = abi_encode(values);
        let start = MemoryPointer.ptr(encoded);
        assert(mload(start + 64) == 0x1234567800000000000000000000000000000000000000000000000000000000);
        let decoded: memory<DynArray<bytes4>> =
        abi_decode(encoded, @memory<DynArray<bytes4>>, @MemoryWordReader);
        assert(Typedef.rep(decoded[1]) == 0xabcdef01);
        return uint256(Typedef.rep(decoded[0]));
    }

    // #[() -> 3]
    function stringsArray() public returns (uint256) {
        let a: memory<string> = "a";
        let b: memory<string> = "bc";
        let values: memory<DynArray<memory<string>>> = [a, b];
        let decoded: memory<DynArray<memory<string>>> =
        abi_decode(abi_encode(values), @memory<DynArray<memory<string>>>, @MemoryWordReader);
        return uint256(strlen(decoded[0]) + strlen(decoded[1]));
    }

    // #[() -> 16]
    function nestedArrays() public returns (uint256) {
        let a: memory<DynArray<uint256>> = [uint256(7)];
        let b: memory<DynArray<uint256>> = [uint256(9)];
        let values: memory<DynArray<memory<DynArray<uint256>>>> = [a, b];
        let decoded: memory<DynArray<memory<DynArray<uint256>>>> =
        abi_decode(abi_encode(values), @memory<DynArray<memory<DynArray<uint256>>>>, @MemoryWordReader);
        return decoded[0][0] + decoded[1][0];
    }
}
