pragma solcore;

//~ specialize: loader std-current
//~ specialize: frontend clean
//~ specialize: entry

pragma no-patterson-condition ABIEncode;

import * from std;
import * from std.dispatch;
import { mload, mstore } from std.opcodes;

// Application defaults precede the standard raw fallback, while concrete
// canonical codecs (including the public uint256 interface) still win.
default impl<t> ABIEncode<t> where t: Typedef<word> {
    function encodeInto(value: t, base: word, offset: word, tail: word) returns (word) {
        mstore(base + offset, Typedef.rep(value) + 100);
        return tail;
    }
}

default impl<t, reader> ABIDecode<ABIDecoder<t, reader>, t> where t: Typedef<word>, reader: WordReader {
    function decode(decoder: ABIDecoder<t, reader>, offset: word) returns (t) {
        return Typedef.abs(WordReader.read(WordReader.advance(decoder, offset)) - 100);
    }
}

contract DefaultRawCodec {
    // #[(7) -> 7]
    function scalarWord(value: uint256) public returns (uint256) {
        let encoded = abi_encode(Typedef.rep(value));
        assert(mload(MemoryPointer.ptr(encoded)) == Typedef.rep(value) + 100);
        let decoded: word = abi_decode(encoded, @word, @MemoryWordReader);
        return uint256(decoded);
    }

    // #[(7) -> 7]
    function scalarByte(value: uint256) public returns (uint256) {
        let encoded = abi_encode(byte(Typedef.rep(value)));
        assert(mload(MemoryPointer.ptr(encoded)) == Typedef.rep(value) + 100);
        let decoded: byte = abi_decode(encoded, @byte, @MemoryWordReader);
        return uint256(Typedef.rep(decoded));
    }

    // #[(107) -> 7]
    function calldataWord(value: uint256) public returns (uint256) {
        let decoder: ABIDecoder<word, CalldataWordReader> = ABIDecoder(CalldataWordReader(4));
        let decoded: word = ABIDecode.decode(decoder, 0);
        return uint256(decoded);
    }

    // #[() -> 3]
    function wordArray() public returns (uint256) {
        let values: memory<DynArray<word>> = [1, 2];
        let encoded = abi_encode(values);
        let start = MemoryPointer.ptr(encoded);
        assert(mload(start + 64) == 101);
        assert(mload(start + 96) == 102);
        let decoded: memory<DynArray<word>> =
        abi_decode(encoded, @memory<DynArray<word>>, @MemoryWordReader);
        return uint256(decoded[0] + decoded[1]);
    }

    // #[() -> 3]
    function byteArray() public returns (uint256) {
        let values: memory<DynArray<byte>> = [byte(1), byte(2)];
        let encoded = abi_encode(values);
        let start = MemoryPointer.ptr(encoded);
        assert(mload(start + 64) == 101);
        assert(mload(start + 96) == 102);
        let decoded: memory<DynArray<byte>> =
        abi_decode(encoded, @memory<DynArray<byte>>, @MemoryWordReader);
        return uint256(Typedef.rep(decoded[0]) + Typedef.rep(decoded[1]));
    }

    // #[() -> 3]
    function nestedArray() public returns (uint256) {
        let empty = allocateDynamicArray(@word, 0);
        let values: memory<DynArray<word>> = [1, 2];
        let nested: memory<DynArray<memory<DynArray<word>>>> = [empty, values];
        let encoded = abi_encode(nested);
        let start = MemoryPointer.ptr(encoded);
        assert(mload(start + 160) == 2);
        assert(mload(start + 192) == 101);
        assert(mload(start + 224) == 102);
        let decoded: memory<DynArray<memory<DynArray<word>>>> =
        abi_decode(encoded, @memory<DynArray<memory<DynArray<word>>>>, @MemoryWordReader);
        assert(mload(Typedef.rep(decoded[0])) == 0);
        return uint256(decoded[1][0] + decoded[1][1]);
    }
}
