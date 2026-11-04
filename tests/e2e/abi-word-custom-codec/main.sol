pragma solcore;

//~ specialize: loader std-current
//~ specialize: frontend clean
//~ specialize: entry

import * from std;
import * from std.dispatch;
import { mload, mstore } from std.opcodes;

// Only std can implement its ABI traits for std's own types, so `word` and
// `byte` always keep std's raw codecs. A word-backed type defined here chooses
// its own ABI attributes and codecs instead, and array elements must retain
// those explicitly selected dictionaries.
enum CustomWord {
    CustomWord(word)
}

impl Typedef<CustomWord, word> {
    function abs(x: word) returns (CustomWord) {
        return CustomWord(x);
    }
    function rep(x: CustomWord) returns (word) {
        match (x) {
            case CustomWord(raw) {
                return raw;
            }
        }
    }
}

impl ABIAttribs<CustomWord> {
    function headSize(ty: Proxy<CustomWord>) returns (word) {
        return 32;
    }
    function isStatic(ty: Proxy<CustomWord>) returns (bool) {
        return true;
    }
}

impl ABIEncode<CustomWord> {
    function encodeInto(value: CustomWord, base: word, offset: word, tail: word) returns (word) {
        mstore(base + offset, Typedef.rep(value) + 100);
        return tail;
    }
}

impl<reader> ABIDecode<ABIDecoder<CustomWord, reader>, CustomWord> where reader: WordReader {
    function decode(decoder: ABIDecoder<CustomWord, reader>, offset: word) returns (CustomWord) {
        return CustomWord(WordReader.read(WordReader.advance(decoder, offset)) - 100);
    }
}

// A second element type with a different codec: each array keeps the codec of
// its own element type.
enum CustomByte {
    CustomByte(byte)
}

impl Typedef<CustomByte, word> {
    function abs(x: word) returns (CustomByte) {
        return CustomByte(byte(x));
    }
    function rep(x: CustomByte) returns (word) {
        match (x) {
            case CustomByte(raw) {
                return Typedef.rep(raw);
            }
        }
    }
}

impl ABIAttribs<CustomByte> {
    function headSize(ty: Proxy<CustomByte>) returns (word) {
        return 32;
    }
    function isStatic(ty: Proxy<CustomByte>) returns (bool) {
        return true;
    }
}

impl ABIEncode<CustomByte> {
    function encodeInto(value: CustomByte, base: word, offset: word, tail: word) returns (word) {
        mstore(base + offset, Typedef.rep(value) + 1000);
        return tail;
    }
}

impl<reader> ABIDecode<ABIDecoder<CustomByte, reader>, CustomByte> where reader: WordReader {
    function decode(decoder: ABIDecoder<CustomByte, reader>, offset: word) returns (CustomByte) {
        return CustomByte(byte(WordReader.read(WordReader.advance(decoder, offset)) - 1000));
    }
}

contract CustomWordCodec {
    // The public interface still uses canonical uint256 codecs, and raw words
    // and bytes keep std's codecs; only this module's own types use custom
    // representations.
    // #[(7) -> 7]
    function scalar(value: uint256) public returns (uint256) {
        let raw: word = Typedef.rep(value);
        assert(mload(MemoryPointer.ptr(abi_encode(raw))) == raw);
        let encoded = abi_encode(CustomWord(raw));
        assert(mload(MemoryPointer.ptr(encoded)) == raw + 100);
        let decoded: CustomWord = abi_decode(encoded, @CustomWord, @MemoryWordReader);
        return uint256(Typedef.rep(decoded));
    }

    // #[() -> 3]
    function array() public returns (uint256) {
        let values: memory<DynArray<CustomWord>> = [CustomWord(1), CustomWord(2)];
        let encoded = abi_encode(values);
        let start = MemoryPointer.ptr(encoded);
        assert(mload(start + 64) == 101);
        assert(mload(start + 96) == 102);
        let decoded: memory<DynArray<CustomWord>> =
        abi_decode(encoded, @memory<DynArray<CustomWord>>, @MemoryWordReader);
        return uint256(Typedef.rep(decoded[0]) + Typedef.rep(decoded[1]));
    }

    // #[(7) -> 7]
    function scalarByte(value: uint256) public returns (uint256) {
        let raw = byte(Typedef.rep(value));
        assert(mload(MemoryPointer.ptr(abi_encode(raw))) == Typedef.rep(value));
        let encoded = abi_encode(CustomByte(raw));
        assert(mload(MemoryPointer.ptr(encoded)) == Typedef.rep(value) + 1000);
        let decoded: CustomByte = abi_decode(encoded, @CustomByte, @MemoryWordReader);
        return uint256(Typedef.rep(decoded));
    }

    // #[() -> 3]
    function byteArray() public returns (uint256) {
        let values: memory<DynArray<CustomByte>> = [CustomByte(byte(1)), CustomByte(byte(2))];
        let encoded = abi_encode(values);
        let start = MemoryPointer.ptr(encoded);
        assert(mload(start + 64) == 1001);
        assert(mload(start + 96) == 1002);
        let decoded: memory<DynArray<CustomByte>> =
        abi_decode(encoded, @memory<DynArray<CustomByte>>, @MemoryWordReader);
        return uint256(Typedef.rep(decoded[0]) + Typedef.rep(decoded[1]));
    }
}
