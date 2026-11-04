pragma solcore;

//~ specialize: loader std-current
//~ specialize: frontend clean
//~ specialize: entry

pragma no-patterson-condition ABIEncode;

import * from std;
import * from std.dispatch;
import { mload, mstore } from std.opcodes;

// Applications cannot add ABI defaults for std's types (a default for every
// word typedef, say), but a type constructor defined here can carry default
// codecs for every word-backed payload. They decide each concrete
// instantiation, including array elements and calldata decoding, while a
// concrete impl still wins over them: Biased<uint256> keeps the canonical
// uint256 encoding.
enum Biased<t> {
    Biased(t)
}

impl<t> Typedef<Biased<t>, word> where t: Typedef<word> {
    function abs(x: word) returns (Biased<t>) {
        return Biased(Typedef.abs(x));
    }
    function rep(x: Biased<t>) returns (word) {
        match (x) {
            case Biased(value) {
                return Typedef.rep(value);
            }
        }
    }
}

default impl<t> ABIEncode<Biased<t>> where t: Typedef<word> {
    function encodeInto(value: Biased<t>, base: word, offset: word, tail: word) returns (word) {
        mstore(base + offset, Typedef.rep(value) + 100);
        return tail;
    }
}

default impl<t, reader> ABIDecode<ABIDecoder<Biased<t>, reader>, Biased<t>> where t: Typedef<word>, reader: WordReader {
    function decode(decoder: ABIDecoder<Biased<t>, reader>, offset: word) returns (Biased<t>) {
        return Typedef.abs(WordReader.read(WordReader.advance(decoder, offset)) - 100);
    }
}

impl ABIEncode<Biased<uint256>> {
    function encodeInto(value: Biased<uint256>, base: word, offset: word, tail: word) returns (word) {
        match (value) {
            case Biased(inner) {
                return ABIEncode.encodeInto(inner, base, offset, tail);
            }
        }
    }
}

impl<reader> ABIDecode<ABIDecoder<Biased<uint256>, reader>, Biased<uint256>> where reader: WordReader {
    function decode(decoder: ABIDecoder<Biased<uint256>, reader>, offset: word) returns (Biased<uint256>) {
        let inner: ABIDecoder<uint256, reader> = ABIDecoder(getReader(decoder));
        let value: uint256 = ABIDecode.decode(inner, offset);
        return Biased(value);
    }
}

contract DefaultRawCodec {
    // #[(7) -> 7]
    function scalarWord(value: uint256) public returns (uint256) {
        let encoded = abi_encode(Biased(Typedef.rep(value)));
        assert(mload(MemoryPointer.ptr(encoded)) == Typedef.rep(value) + 100);
        let decoded: Biased<word> = abi_decode(encoded, @Biased<word>, @MemoryWordReader);
        return uint256(Typedef.rep(decoded));
    }

    // #[(7) -> 7]
    function scalarByte(value: uint256) public returns (uint256) {
        let encoded = abi_encode(Biased(byte(Typedef.rep(value))));
        assert(mload(MemoryPointer.ptr(encoded)) == Typedef.rep(value) + 100);
        let decoded: Biased<byte> = abi_decode(encoded, @Biased<byte>, @MemoryWordReader);
        return uint256(Typedef.rep(decoded));
    }

    // The concrete impl, not the defaults, encodes Biased<uint256>.
    // #[(7) -> 7]
    function scalarUint(value: uint256) public returns (uint256) {
        let encoded = abi_encode(Biased(value));
        assert(mload(MemoryPointer.ptr(encoded)) == Typedef.rep(value));
        let decoded: Biased<uint256> = abi_decode(encoded, @Biased<uint256>, @MemoryWordReader);
        return uint256(Typedef.rep(decoded));
    }

    // #[(107) -> 7]
    function calldataWord(value: uint256) public returns (uint256) {
        let decoder: ABIDecoder<Biased<word>, CalldataWordReader> = ABIDecoder(CalldataWordReader(4));
        let decoded: Biased<word> = ABIDecode.decode(decoder, 0);
        return uint256(Typedef.rep(decoded));
    }

    // #[() -> 3]
    function wordArray() public returns (uint256) {
        let values: memory<DynArray<Biased<word>>> = [Biased(1), Biased(2)];
        let encoded = abi_encode(values);
        let start = MemoryPointer.ptr(encoded);
        assert(mload(start + 64) == 101);
        assert(mload(start + 96) == 102);
        let decoded: memory<DynArray<Biased<word>>> =
        abi_decode(encoded, @memory<DynArray<Biased<word>>>, @MemoryWordReader);
        return uint256(Typedef.rep(decoded[0]) + Typedef.rep(decoded[1]));
    }

    // #[() -> 3]
    function byteArray() public returns (uint256) {
        let values: memory<DynArray<Biased<byte>>> = [Biased(byte(1)), Biased(byte(2))];
        let encoded = abi_encode(values);
        let start = MemoryPointer.ptr(encoded);
        assert(mload(start + 64) == 101);
        assert(mload(start + 96) == 102);
        let decoded: memory<DynArray<Biased<byte>>> =
        abi_decode(encoded, @memory<DynArray<Biased<byte>>>, @MemoryWordReader);
        return uint256(Typedef.rep(decoded[0]) + Typedef.rep(decoded[1]));
    }

    // #[() -> 3]
    function nestedArray() public returns (uint256) {
        let empty = allocateDynamicArray(@Biased<word>, 0);
        let values: memory<DynArray<Biased<word>>> = [Biased(1), Biased(2)];
        let nested: memory<DynArray<memory<DynArray<Biased<word>>>>> = [empty, values];
        let encoded = abi_encode(nested);
        let start = MemoryPointer.ptr(encoded);
        assert(mload(start + 160) == 2);
        assert(mload(start + 192) == 101);
        assert(mload(start + 224) == 102);
        let decoded: memory<DynArray<memory<DynArray<Biased<word>>>>> =
        abi_decode(encoded, @memory<DynArray<memory<DynArray<Biased<word>>>>>, @MemoryWordReader);
        assert(mload(Typedef.rep(decoded[0])) == 0);
        return uint256(Typedef.rep(decoded[1][0]) + Typedef.rep(decoded[1][1]));
    }

    // #[() -> 3]
    function uintArray() public returns (uint256) {
        let values: memory<DynArray<Biased<uint256>>> = [Biased(uint256(1)), Biased(uint256(2))];
        let encoded = abi_encode(values);
        let start = MemoryPointer.ptr(encoded);
        assert(mload(start + 64) == 1);
        assert(mload(start + 96) == 2);
        let decoded: memory<DynArray<Biased<uint256>>> =
        abi_decode(encoded, @memory<DynArray<Biased<uint256>>>, @MemoryWordReader);
        return uint256(Typedef.rep(decoded[0]) + Typedef.rep(decoded[1]));
    }
}
