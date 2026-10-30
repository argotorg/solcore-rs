pragma solcore;

//~ specialize: loader std-current
//~ specialize: frontend clean
//~ specialize: entry

import * from std;
import * from std.dispatch;
import { mload, mstore } from std.opcodes;

// Applications could supply raw-word codecs before the standard fallback
// existed. Array elements must retain those explicitly selected dictionaries.
impl ABIEncode<word> {
    function encodeInto(value: word, base: word, offset: word, tail: word) returns (word) {
        mstore(base + offset, value + 100);
        return tail;
    }
}

impl<reader> ABIDecode<ABIDecoder<word, reader>, word> where reader: WordReader {
    function decode(decoder: ABIDecoder<word, reader>, offset: word) returns (word) {
        return WordReader.read(WordReader.advance(decoder, offset)) - 100;
    }
}

impl ABIEncode<byte> {
    function encodeInto(value: byte, base: word, offset: word, tail: word) returns (word) {
        mstore(base + offset, Typedef.rep(value) + 1000);
        return tail;
    }
}

impl<reader> ABIDecode<ABIDecoder<byte, reader>, byte> where reader: WordReader {
    function decode(decoder: ABIDecoder<byte, reader>, offset: word) returns (byte) {
        return byte(WordReader.read(WordReader.advance(decoder, offset)) - 1000);
    }
}

contract CustomWordCodec {
    // The public interface still uses canonical uint256 codecs; only internal
    // raw-word values use the application-specific representation.
    // #[(7) -> 7]
    function scalar(value: uint256) public returns (uint256) {
        let encoded = abi_encode(Typedef.rep(value));
        assert(mload(MemoryPointer.ptr(encoded)) == Typedef.rep(value) + 100);
        let decoded: word = abi_decode(encoded, @word, @MemoryWordReader);
        return uint256(decoded);
    }

    // #[() -> 3]
    function array() public returns (uint256) {
        let values: memory<DynArray<word>> = [1, 2];
        let encoded = abi_encode(values);
        let start = MemoryPointer.ptr(encoded);
        assert(mload(start + 64) == 101);
        assert(mload(start + 96) == 102);
        let decoded: memory<DynArray<word>> =
        abi_decode(encoded, @memory<DynArray<word>>, @MemoryWordReader);
        return uint256(decoded[0] + decoded[1]);
    }

    // #[(7) -> 7]
    function scalarByte(value: uint256) public returns (uint256) {
        let encoded = abi_encode(byte(Typedef.rep(value)));
        assert(mload(MemoryPointer.ptr(encoded)) == Typedef.rep(value) + 1000);
        let decoded: byte = abi_decode(encoded, @byte, @MemoryWordReader);
        return uint256(Typedef.rep(decoded));
    }

    // #[() -> 3]
    function byteArray() public returns (uint256) {
        let values: memory<DynArray<byte>> = [byte(1), byte(2)];
        let encoded = abi_encode(values);
        let start = MemoryPointer.ptr(encoded);
        assert(mload(start + 64) == 1001);
        assert(mload(start + 96) == 1002);
        let decoded: memory<DynArray<byte>> =
        abi_decode(encoded, @memory<DynArray<byte>>, @MemoryWordReader);
        return uint256(Typedef.rep(decoded[0]) + Typedef.rep(decoded[1]));
    }
}
