pragma solcore;

// std.unsigned: unsigned integer types uint8..uint128 (uint256 is in std).
// Each type wraps a word. Arithmetic is checked; bitwise operations and ABI
// encoding mask to the selected width. Typedef.abs preserves the raw
// representation; ABI decoding validates it.
import * from std;
import { SigString } from std.dispatch;
import { mstore, sstore, sload } from std.opcodes;

// The shift amount type is open (any `ShiftAmount`), as in std.sol.
pragma no-coverage-condition Shl, Shr;

export { uint8, uint16, uint32, uint64, uint128 };

// --- uint8 (8-bit unsigned) ---
struct uint8 {
    value: word;
}

impl Typedef<uint8, word> {
    function abs(w: word) returns (uint8) {
        return uint8(w);
    }
    function rep(x: uint8) returns (word) {
        return x.value;
    }
}
impl Add<uint8> {
    function add(x: uint8, y: uint8) returns (uint8) {
        return Typedef.abs(uAddChecked(Typedef.rep(x), Typedef.rep(y), 0xff));
    }
}
impl Sub<uint8> {
    function sub(x: uint8, y: uint8) returns (uint8) {
        return Typedef.abs(uSubChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}
impl Mul<uint8> {
    function mul(x: uint8, y: uint8) returns (uint8) {
        return Typedef.abs(uMulChecked(Typedef.rep(x), Typedef.rep(y), 0xff));
    }
}
impl Div<uint8> {
    function div(x: uint8, y: uint8) returns (uint8) {
        return Typedef.abs(uDivChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}
impl Mod<uint8> {
    function mod(x: uint8, y: uint8) returns (uint8) {
        return Typedef.abs(uModChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}
impl BitAnd<uint8> {
    function band(x: uint8, y: uint8) returns (uint8) {
        return Typedef.abs(BitAnd.band(BitAnd.band(Typedef.rep(x), Typedef.rep(y)), 0xff));
    }
}
impl BitOr<uint8> {
    function bor(x: uint8, y: uint8) returns (uint8) {
        return Typedef.abs(BitAnd.band(BitOr.bor(Typedef.rep(x), Typedef.rep(y)), 0xff));
    }
}
impl BitXor<uint8> {
    function bxor(x: uint8, y: uint8) returns (uint8) {
        return Typedef.abs(BitAnd.band(BitXor.bxor(Typedef.rep(x), Typedef.rep(y)), 0xff));
    }
}
impl BitNot<uint8> {
    function bnot(x: uint8) returns (uint8) {
        return Typedef.abs(BitAnd.band(BitNot.bnot(Typedef.rep(x)), 0xff));
    }
}
impl ShiftAmount<uint8> {
    function amount(x: uint8) returns (word) {
        return Typedef.rep(x);
    }
}
// Bits shifted past the width are discarded.
impl<n> Shl<uint8, n> where n: ShiftAmount {
    function bshl(l: uint8, r: n) returns (uint8) {
        return Typedef.abs(BitAnd.band(Shl.bshl(Typedef.rep(l), r), 0xff));
    }
}
impl<n> Shr<uint8, n> where n: ShiftAmount {
    function bshr(l: uint8, r: n) returns (uint8) {
        return Typedef.abs(Shr.bshr(Typedef.rep(l), r));
    }
}
impl Eq<uint8> {
    function eq(x: uint8, y: uint8) returns (bool) {
        return Eq.eq(Typedef.rep(x), Typedef.rep(y));
    }
}
impl Ord<uint8> {
    function gt(x: uint8, y: uint8) returns (bool) {
        return Ord.gt(Typedef.rep(x), Typedef.rep(y));
    }
}
impl Bounded<uint8> {
    function minVal() returns (uint8) {
        return uint8(0x0);
    }
    function maxVal() returns (uint8) {
        return uint8(0xff);
    }
}
impl Int<uint8> {
    function fromInteger(comptime x: integer) returns (uint8) {
        let checked: comptime<word> = checkedIntegerWord(x, 0, 255);
        return uint8(checked);
    }
}
impl SigString<uint8> {
    function sigStr(x: Proxy<uint8>) returns (string) {
        return "uint8";
    }
}
impl ABIAttribs<uint8> {
    function headSize(ty: Proxy<uint8>) returns (word) {
        return 32;
    }
    function isStatic(ty: Proxy<uint8>) returns (bool) {
        return true;
    }
}
impl ABIEncode<uint8> {
    function encodeInto(x: uint8, basePtr: word, offset: word, tail: word) returns (word) {
        mstore(basePtr + offset, BitAnd.band(Typedef.rep(x), 0xff));
        return tail;
    }
}
impl<reader> ABIDecode<ABIDecoder<uint8, reader>, uint8> where reader: WordReader {
    function decode(ptr: ABIDecoder<uint8, reader>, currentHeadOffset: word) returns (uint8) {
        let raw = WordReader.read(WordReader.advance(ptr, currentHeadOffset));
        require(raw == BitAnd.band(raw, 0xff), Error(0x6cc54a63)); // DirtyHigherBitsForInteger()
        return Typedef.abs(raw);
    }
}
impl StorageSize<uint8> {
    function size(x: Proxy<uint8>) returns (word) {
        return 1;
    }
}
impl StorageType<uint8> {
    function load(ptr: word) returns (uint8) {
        return uint8(StorageType.load(ptr));
    }
    function store(ptr: word, value: uint8) returns (()) {
        StorageType.store(ptr, Typedef.rep(value));
    }
}
impl CanStore<storage<uint8>, uint8> {
    function store(l: storage<uint8>, r: uint8) returns (()) {
        StorageType.store(Typedef.rep(l), r);
    }
    function load(l: storage<uint8>) returns (uint8) {
        return StorageType.load(Typedef.rep(l));
    }
}
impl StorageCopy<uint8> {
    function copySlot(dst: storage<uint8>, src: storage<uint8>) returns (()) {
        sstore(Typedef.rep(dst), sload(Typedef.rep(src)));
    }
}

// --- uint16 (16-bit unsigned) ---
struct uint16 {
    value: word;
}
impl Typedef<uint16, word> {
    function abs(w: word) returns (uint16) {
        return uint16(w);
    }
    function rep(x: uint16) returns (word) {
        return x.value;
    }
}
impl Add<uint16> {
    function add(x: uint16, y: uint16) returns (uint16) {
        return Typedef.abs(uAddChecked(Typedef.rep(x), Typedef.rep(y), 0xffff));
    }
}
impl Sub<uint16> {
    function sub(x: uint16, y: uint16) returns (uint16) {
        return Typedef.abs(uSubChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}
impl Mul<uint16> {
    function mul(x: uint16, y: uint16) returns (uint16) {
        return Typedef.abs(uMulChecked(Typedef.rep(x), Typedef.rep(y), 0xffff));
    }
}
impl Div<uint16> {
    function div(x: uint16, y: uint16) returns (uint16) {
        return Typedef.abs(uDivChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}
impl Mod<uint16> {
    function mod(x: uint16, y: uint16) returns (uint16) {
        return Typedef.abs(uModChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}
impl BitAnd<uint16> {
    function band(x: uint16, y: uint16) returns (uint16) {
        return Typedef.abs(BitAnd.band(BitAnd.band(Typedef.rep(x), Typedef.rep(y)), 0xffff));
    }
}
impl BitOr<uint16> {
    function bor(x: uint16, y: uint16) returns (uint16) {
        return Typedef.abs(BitAnd.band(BitOr.bor(Typedef.rep(x), Typedef.rep(y)), 0xffff));
    }
}
impl BitXor<uint16> {
    function bxor(x: uint16, y: uint16) returns (uint16) {
        return Typedef.abs(BitAnd.band(BitXor.bxor(Typedef.rep(x), Typedef.rep(y)), 0xffff));
    }
}
impl BitNot<uint16> {
    function bnot(x: uint16) returns (uint16) {
        return Typedef.abs(BitAnd.band(BitNot.bnot(Typedef.rep(x)), 0xffff));
    }
}
impl ShiftAmount<uint16> {
    function amount(x: uint16) returns (word) {
        return Typedef.rep(x);
    }
}
// Bits shifted past the width are discarded.
impl<n> Shl<uint16, n> where n: ShiftAmount {
    function bshl(l: uint16, r: n) returns (uint16) {
        return Typedef.abs(BitAnd.band(Shl.bshl(Typedef.rep(l), r), 0xffff));
    }
}
impl<n> Shr<uint16, n> where n: ShiftAmount {
    function bshr(l: uint16, r: n) returns (uint16) {
        return Typedef.abs(Shr.bshr(Typedef.rep(l), r));
    }
}
impl Eq<uint16> {
    function eq(x: uint16, y: uint16) returns (bool) {
        return Eq.eq(Typedef.rep(x), Typedef.rep(y));
    }
}
impl Ord<uint16> {
    function gt(x: uint16, y: uint16) returns (bool) {
        return Ord.gt(Typedef.rep(x), Typedef.rep(y));
    }
}
impl Bounded<uint16> {
    function minVal() returns (uint16) {
        return uint16(0x0);
    }
    function maxVal() returns (uint16) {
        return uint16(0xffff);
    }
}
impl Int<uint16> {
    function fromInteger(comptime x: integer) returns (uint16) {
        let checked: comptime<word> = checkedIntegerWord(x, 0, 65535);
        return uint16(checked);
    }
}
impl SigString<uint16> {
    function sigStr(x: Proxy<uint16>) returns (string) {
        return "uint16";
    }
}
impl ABIAttribs<uint16> {
    function headSize(ty: Proxy<uint16>) returns (word) {
        return 32;
    }
    function isStatic(ty: Proxy<uint16>) returns (bool) {
        return true;
    }
}
impl ABIEncode<uint16> {
    function encodeInto(x: uint16, basePtr: word, offset: word, tail: word) returns (word) {
        mstore(basePtr + offset, BitAnd.band(Typedef.rep(x), 0xffff));
        return tail;
    }
}
impl<reader> ABIDecode<ABIDecoder<uint16, reader>, uint16> where reader: WordReader {
    function decode(ptr: ABIDecoder<uint16, reader>, currentHeadOffset: word) returns (uint16) {
        let raw = WordReader.read(WordReader.advance(ptr, currentHeadOffset));
        require(raw == BitAnd.band(raw, 0xffff), Error(0x6cc54a63)); // DirtyHigherBitsForInteger()
        return Typedef.abs(raw);
    }
}
impl StorageSize<uint16> {
    function size(x: Proxy<uint16>) returns (word) {
        return 1;
    }
}
impl StorageType<uint16> {
    function load(ptr: word) returns (uint16) {
        return uint16(StorageType.load(ptr));
    }
    function store(ptr: word, value: uint16) returns (()) {
        StorageType.store(ptr, Typedef.rep(value));
    }
}
impl CanStore<storage<uint16>, uint16> {
    function store(l: storage<uint16>, r: uint16) returns (()) {
        StorageType.store(Typedef.rep(l), r);
    }
    function load(l: storage<uint16>) returns (uint16) {
        return StorageType.load(Typedef.rep(l));
    }
}
impl StorageCopy<uint16> {
    function copySlot(dst: storage<uint16>, src: storage<uint16>) returns (()) {
        sstore(Typedef.rep(dst), sload(Typedef.rep(src)));
    }
}

// --- uint32 (32-bit unsigned) ---
struct uint32 {
    value: word;
}
impl Typedef<uint32, word> {
    function abs(w: word) returns (uint32) {
        return uint32(w);
    }
    function rep(x: uint32) returns (word) {
        return x.value;
    }
}
impl Add<uint32> {
    function add(x: uint32, y: uint32) returns (uint32) {
        return Typedef.abs(uAddChecked(Typedef.rep(x), Typedef.rep(y), 0xffffffff));
    }
}
impl Sub<uint32> {
    function sub(x: uint32, y: uint32) returns (uint32) {
        return Typedef.abs(uSubChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}
impl Mul<uint32> {
    function mul(x: uint32, y: uint32) returns (uint32) {
        return Typedef.abs(uMulChecked(Typedef.rep(x), Typedef.rep(y), 0xffffffff));
    }
}
impl Div<uint32> {
    function div(x: uint32, y: uint32) returns (uint32) {
        return Typedef.abs(uDivChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}
impl Mod<uint32> {
    function mod(x: uint32, y: uint32) returns (uint32) {
        return Typedef.abs(uModChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}
impl BitAnd<uint32> {
    function band(x: uint32, y: uint32) returns (uint32) {
        return Typedef.abs(BitAnd.band(BitAnd.band(Typedef.rep(x), Typedef.rep(y)), 0xffffffff));
    }
}
impl BitOr<uint32> {
    function bor(x: uint32, y: uint32) returns (uint32) {
        return Typedef.abs(BitAnd.band(BitOr.bor(Typedef.rep(x), Typedef.rep(y)), 0xffffffff));
    }
}
impl BitXor<uint32> {
    function bxor(x: uint32, y: uint32) returns (uint32) {
        return Typedef.abs(BitAnd.band(BitXor.bxor(Typedef.rep(x), Typedef.rep(y)), 0xffffffff));
    }
}
impl BitNot<uint32> {
    function bnot(x: uint32) returns (uint32) {
        return Typedef.abs(BitAnd.band(BitNot.bnot(Typedef.rep(x)), 0xffffffff));
    }
}
impl ShiftAmount<uint32> {
    function amount(x: uint32) returns (word) {
        return Typedef.rep(x);
    }
}
// Bits shifted past the width are discarded.
impl<n> Shl<uint32, n> where n: ShiftAmount {
    function bshl(l: uint32, r: n) returns (uint32) {
        return Typedef.abs(BitAnd.band(Shl.bshl(Typedef.rep(l), r), 0xffffffff));
    }
}
impl<n> Shr<uint32, n> where n: ShiftAmount {
    function bshr(l: uint32, r: n) returns (uint32) {
        return Typedef.abs(Shr.bshr(Typedef.rep(l), r));
    }
}
impl Eq<uint32> {
    function eq(x: uint32, y: uint32) returns (bool) {
        return Eq.eq(Typedef.rep(x), Typedef.rep(y));
    }
}
impl Ord<uint32> {
    function gt(x: uint32, y: uint32) returns (bool) {
        return Ord.gt(Typedef.rep(x), Typedef.rep(y));
    }
}
impl Bounded<uint32> {
    function minVal() returns (uint32) {
        return uint32(0x0);
    }
    function maxVal() returns (uint32) {
        return uint32(0xffffffff);
    }
}
impl Int<uint32> {
    function fromInteger(comptime x: integer) returns (uint32) {
        let checked: comptime<word> = checkedIntegerWord(x, 0, 4294967295);
        return uint32(checked);
    }
}
impl SigString<uint32> {
    function sigStr(x: Proxy<uint32>) returns (string) {
        return "uint32";
    }
}
impl ABIAttribs<uint32> {
    function headSize(ty: Proxy<uint32>) returns (word) {
        return 32;
    }
    function isStatic(ty: Proxy<uint32>) returns (bool) {
        return true;
    }
}
impl ABIEncode<uint32> {
    function encodeInto(x: uint32, basePtr: word, offset: word, tail: word) returns (word) {
        mstore(basePtr + offset, BitAnd.band(Typedef.rep(x), 0xffffffff));
        return tail;
    }
}
impl<reader> ABIDecode<ABIDecoder<uint32, reader>, uint32> where reader: WordReader {
    function decode(ptr: ABIDecoder<uint32, reader>, currentHeadOffset: word) returns (uint32) {
        let raw = WordReader.read(WordReader.advance(ptr, currentHeadOffset));
        require(raw == BitAnd.band(raw, 0xffffffff), Error(0x6cc54a63)); // DirtyHigherBitsForInteger()
        return Typedef.abs(raw);
    }
}
impl StorageSize<uint32> {
    function size(x: Proxy<uint32>) returns (word) {
        return 1;
    }
}
impl StorageType<uint32> {
    function load(ptr: word) returns (uint32) {
        return uint32(StorageType.load(ptr));
    }
    function store(ptr: word, value: uint32) returns (()) {
        StorageType.store(ptr, Typedef.rep(value));
    }
}
impl CanStore<storage<uint32>, uint32> {
    function store(l: storage<uint32>, r: uint32) returns (()) {
        StorageType.store(Typedef.rep(l), r);
    }
    function load(l: storage<uint32>) returns (uint32) {
        return StorageType.load(Typedef.rep(l));
    }
}
impl StorageCopy<uint32> {
    function copySlot(dst: storage<uint32>, src: storage<uint32>) returns (()) {
        sstore(Typedef.rep(dst), sload(Typedef.rep(src)));
    }
}

// --- uint64 (64-bit unsigned) ---
struct uint64 {
    value: word;
}
impl Typedef<uint64, word> {
    function abs(w: word) returns (uint64) {
        return uint64(w);
    }
    function rep(x: uint64) returns (word) {
        return x.value;
    }
}
impl Add<uint64> {
    function add(x: uint64, y: uint64) returns (uint64) {
        return Typedef.abs(uAddChecked(Typedef.rep(x), Typedef.rep(y), 0xffffffffffffffff));
    }
}
impl Sub<uint64> {
    function sub(x: uint64, y: uint64) returns (uint64) {
        return Typedef.abs(uSubChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}
impl Mul<uint64> {
    function mul(x: uint64, y: uint64) returns (uint64) {
        return Typedef.abs(uMulChecked(Typedef.rep(x), Typedef.rep(y), 0xffffffffffffffff));
    }
}
impl Div<uint64> {
    function div(x: uint64, y: uint64) returns (uint64) {
        return Typedef.abs(uDivChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}
impl Mod<uint64> {
    function mod(x: uint64, y: uint64) returns (uint64) {
        return Typedef.abs(uModChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}
impl BitAnd<uint64> {
    function band(x: uint64, y: uint64) returns (uint64) {
        return Typedef.abs(BitAnd.band(BitAnd.band(Typedef.rep(x), Typedef.rep(y)), 0xffffffffffffffff));
    }
}
impl BitOr<uint64> {
    function bor(x: uint64, y: uint64) returns (uint64) {
        return Typedef.abs(BitAnd.band(BitOr.bor(Typedef.rep(x), Typedef.rep(y)), 0xffffffffffffffff));
    }
}
impl BitXor<uint64> {
    function bxor(x: uint64, y: uint64) returns (uint64) {
        return Typedef.abs(BitAnd.band(BitXor.bxor(Typedef.rep(x), Typedef.rep(y)), 0xffffffffffffffff));
    }
}
impl BitNot<uint64> {
    function bnot(x: uint64) returns (uint64) {
        return Typedef.abs(BitAnd.band(BitNot.bnot(Typedef.rep(x)), 0xffffffffffffffff));
    }
}
impl ShiftAmount<uint64> {
    function amount(x: uint64) returns (word) {
        return Typedef.rep(x);
    }
}
// Bits shifted past the width are discarded.
impl<n> Shl<uint64, n> where n: ShiftAmount {
    function bshl(l: uint64, r: n) returns (uint64) {
        return Typedef.abs(BitAnd.band(Shl.bshl(Typedef.rep(l), r), 0xffffffffffffffff));
    }
}
impl<n> Shr<uint64, n> where n: ShiftAmount {
    function bshr(l: uint64, r: n) returns (uint64) {
        return Typedef.abs(Shr.bshr(Typedef.rep(l), r));
    }
}
impl Eq<uint64> {
    function eq(x: uint64, y: uint64) returns (bool) {
        return Eq.eq(Typedef.rep(x), Typedef.rep(y));
    }
}
impl Ord<uint64> {
    function gt(x: uint64, y: uint64) returns (bool) {
        return Ord.gt(Typedef.rep(x), Typedef.rep(y));
    }
}
impl Bounded<uint64> {
    function minVal() returns (uint64) {
        return uint64(0x0);
    }
    function maxVal() returns (uint64) {
        return uint64(0xffffffffffffffff);
    }
}
impl Int<uint64> {
    function fromInteger(comptime x: integer) returns (uint64) {
        let checked: comptime<word> = checkedIntegerWord(x, 0, 18446744073709551615);
        return uint64(checked);
    }
}
impl SigString<uint64> {
    function sigStr(x: Proxy<uint64>) returns (string) {
        return "uint64";
    }
}
impl ABIAttribs<uint64> {
    function headSize(ty: Proxy<uint64>) returns (word) {
        return 32;
    }
    function isStatic(ty: Proxy<uint64>) returns (bool) {
        return true;
    }
}
impl ABIEncode<uint64> {
    function encodeInto(x: uint64, basePtr: word, offset: word, tail: word) returns (word) {
        mstore(basePtr + offset, BitAnd.band(Typedef.rep(x), 0xffffffffffffffff));
        return tail;
    }
}
impl<reader> ABIDecode<ABIDecoder<uint64, reader>, uint64> where reader: WordReader {
    function decode(ptr: ABIDecoder<uint64, reader>, currentHeadOffset: word) returns (uint64) {
        let raw = WordReader.read(WordReader.advance(ptr, currentHeadOffset));
        require(raw == BitAnd.band(raw, 0xffffffffffffffff), Error(0x6cc54a63)); // DirtyHigherBitsForInteger()
        return Typedef.abs(raw);
    }
}
impl StorageSize<uint64> {
    function size(x: Proxy<uint64>) returns (word) {
        return 1;
    }
}
impl StorageType<uint64> {
    function load(ptr: word) returns (uint64) {
        return uint64(StorageType.load(ptr));
    }
    function store(ptr: word, value: uint64) returns (()) {
        StorageType.store(ptr, Typedef.rep(value));
    }
}
impl CanStore<storage<uint64>, uint64> {
    function store(l: storage<uint64>, r: uint64) returns (()) {
        StorageType.store(Typedef.rep(l), r);
    }
    function load(l: storage<uint64>) returns (uint64) {
        return StorageType.load(Typedef.rep(l));
    }
}
impl StorageCopy<uint64> {
    function copySlot(dst: storage<uint64>, src: storage<uint64>) returns (()) {
        sstore(Typedef.rep(dst), sload(Typedef.rep(src)));
    }
}

// --- uint128 (128-bit unsigned) ---
struct uint128 {
    value: word;
}
impl Typedef<uint128, word> {
    function abs(w: word) returns (uint128) {
        return uint128(w);
    }
    function rep(x: uint128) returns (word) {
        return x.value;
    }
}
impl Add<uint128> {
    function add(x: uint128, y: uint128) returns (uint128) {
        return Typedef.abs(uAddChecked(Typedef.rep(x), Typedef.rep(y), 0xffffffffffffffffffffffffffffffff));
    }
}
impl Sub<uint128> {
    function sub(x: uint128, y: uint128) returns (uint128) {
        return Typedef.abs(uSubChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}
impl Mul<uint128> {
    function mul(x: uint128, y: uint128) returns (uint128) {
        return Typedef.abs(uMulChecked(Typedef.rep(x), Typedef.rep(y), 0xffffffffffffffffffffffffffffffff));
    }
}
impl Div<uint128> {
    function div(x: uint128, y: uint128) returns (uint128) {
        return Typedef.abs(uDivChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}
impl Mod<uint128> {
    function mod(x: uint128, y: uint128) returns (uint128) {
        return Typedef.abs(uModChecked(Typedef.rep(x), Typedef.rep(y)));
    }
}
impl BitAnd<uint128> {
    function band(x: uint128, y: uint128) returns (uint128) {
        return Typedef.abs(BitAnd.band(BitAnd.band(Typedef.rep(x), Typedef.rep(y)), 0xffffffffffffffffffffffffffffffff));
    }
}
impl BitOr<uint128> {
    function bor(x: uint128, y: uint128) returns (uint128) {
        return Typedef.abs(BitAnd.band(BitOr.bor(Typedef.rep(x), Typedef.rep(y)), 0xffffffffffffffffffffffffffffffff));
    }
}
impl BitXor<uint128> {
    function bxor(x: uint128, y: uint128) returns (uint128) {
        return Typedef.abs(BitAnd.band(BitXor.bxor(Typedef.rep(x), Typedef.rep(y)), 0xffffffffffffffffffffffffffffffff));
    }
}
impl BitNot<uint128> {
    function bnot(x: uint128) returns (uint128) {
        return Typedef.abs(BitAnd.band(BitNot.bnot(Typedef.rep(x)), 0xffffffffffffffffffffffffffffffff));
    }
}
impl ShiftAmount<uint128> {
    function amount(x: uint128) returns (word) {
        return Typedef.rep(x);
    }
}
// Bits shifted past the width are discarded.
impl<n> Shl<uint128, n> where n: ShiftAmount {
    function bshl(l: uint128, r: n) returns (uint128) {
        return Typedef.abs(BitAnd.band(Shl.bshl(Typedef.rep(l), r), 0xffffffffffffffffffffffffffffffff));
    }
}
impl<n> Shr<uint128, n> where n: ShiftAmount {
    function bshr(l: uint128, r: n) returns (uint128) {
        return Typedef.abs(Shr.bshr(Typedef.rep(l), r));
    }
}
impl Eq<uint128> {
    function eq(x: uint128, y: uint128) returns (bool) {
        return Eq.eq(Typedef.rep(x), Typedef.rep(y));
    }
}
impl Ord<uint128> {
    function gt(x: uint128, y: uint128) returns (bool) {
        return Ord.gt(Typedef.rep(x), Typedef.rep(y));
    }
}
impl Bounded<uint128> {
    function minVal() returns (uint128) {
        return uint128(0x0);
    }
    function maxVal() returns (uint128) {
        return uint128(0xffffffffffffffffffffffffffffffff);
    }
}
impl Int<uint128> {
    function fromInteger(comptime x: integer) returns (uint128) {
        let checked: comptime<word> = checkedIntegerWord(x, 0, 340282366920938463463374607431768211455);
        return uint128(checked);
    }
}
impl SigString<uint128> {
    function sigStr(x: Proxy<uint128>) returns (string) {
        return "uint128";
    }
}
impl ABIAttribs<uint128> {
    function headSize(ty: Proxy<uint128>) returns (word) {
        return 32;
    }
    function isStatic(ty: Proxy<uint128>) returns (bool) {
        return true;
    }
}
impl ABIEncode<uint128> {
    function encodeInto(x: uint128, basePtr: word, offset: word, tail: word) returns (word) {
        mstore(basePtr + offset, BitAnd.band(Typedef.rep(x), 0xffffffffffffffffffffffffffffffff));
        return tail;
    }
}
impl<reader> ABIDecode<ABIDecoder<uint128, reader>, uint128> where reader: WordReader {
    function decode(ptr: ABIDecoder<uint128, reader>, currentHeadOffset: word) returns (uint128) {
        let raw = WordReader.read(WordReader.advance(ptr, currentHeadOffset));
        require(raw == BitAnd.band(raw, 0xffffffffffffffffffffffffffffffff), Error(0x6cc54a63)); // DirtyHigherBitsForInteger()
        return Typedef.abs(raw);
    }
}
impl StorageSize<uint128> {
    function size(x: Proxy<uint128>) returns (word) {
        return 1;
    }
}
impl StorageType<uint128> {
    function load(ptr: word) returns (uint128) {
        return uint128(StorageType.load(ptr));
    }
    function store(ptr: word, value: uint128) returns (()) {
        StorageType.store(ptr, Typedef.rep(value));
    }
}
impl CanStore<storage<uint128>, uint128> {
    function store(l: storage<uint128>, r: uint128) returns (()) {
        StorageType.store(Typedef.rep(l), r);
    }
    function load(l: storage<uint128>) returns (uint128) {
        return StorageType.load(Typedef.rep(l));
    }
}
impl StorageCopy<uint128> {
    function copySlot(dst: storage<uint128>, src: storage<uint128>) returns (()) {
        sstore(Typedef.rep(dst), sload(Typedef.rep(src)));
    }
}
