pragma solcore;

export { memory(*), storage(*), calldata(*), DynArray, array(*), uint256(*), address(*), string, Encoded(*), Decoded(*), concatLit, Add, Array, ArrayPush, Length, Typedef, CanStore, RValueIdxAccess };

enum memory<t> {
    memory(word)
}
enum storage<t> {
    storage(word)
}
enum calldata<t> {
    calldata(word)
}
enum DynArray<t> {}
enum array<t> {
    array(word)
}
enum uint256 {
    uint256(word)
}
enum address {
    address(word)
}
enum string {}
enum Encoded {
    Encoded(word)
}
enum Decoded {
    Decoded(word)
}

function concatLit(comptime lhs: string, comptime rhs: string) returns (string) {
    return lhs;
}

trait Add<self> {
    function add(lhs: self, rhs: self) returns (self);
}

impl Add<uint256> {
    function add(lhs: uint256, rhs: uint256) returns (uint256) {
        return lhs;
    }
}

trait Typedef<abs, rep> {
    function abs(x: rep) returns (abs);
    function rep(x: abs) returns (rep);
}

default impl<t> Typedef<t, t> {
    function abs(x: t) returns (t) {
        return x;
    }
    function rep(x: t) returns (t) {
        return x;
    }
}

impl Typedef<uint256, word> {
    function abs(x: word) returns (uint256) {
        return uint256(x);
    }
    function rep(x: uint256) returns (word) {
        return 0;
    }
}

impl Typedef<memory<string>, word> {
    function abs(x: word) returns (memory<string>) {
        return memory(x);
    }
    function rep(x: memory<string>) returns (word) {
        return 0;
    }
}

trait RValueIdxAccess<col_idx, val> {
    function lookup(xi: col_idx) returns (val);
}

impl<i> RValueIdxAccess<(calldata<array<Encoded>>, i), Decoded> where i: Typedef<word> {
    function lookup(xi: (calldata<array<Encoded>>, i)) returns (Decoded) {
        return Decoded(0);
    }
}

trait CanStore<dst, value> {
    function store(dst: dst, value: value) returns ();
    function load(dst: dst) returns (value);
}

impl CanStore<storage<word>, word> {
    function store(dst: storage<word>, value: word) returns () {
        return ();
    }
    function load(dst: storage<word>) returns (word) {
        return 0;
    }
}

impl CanStore<storage<uint256>, uint256> {
    function store(dst: storage<uint256>, value: uint256) returns () {
        return ();
    }
    function load(dst: storage<uint256>) returns (uint256) {
        return uint256(0);
    }
}

impl CanStore<storage<string>, memory<string>> {
    function store(dst: storage<string>, value: memory<string>) returns () {
        return ();
    }
    function load(dst: storage<string>) returns (memory<string>) {
        return memory(0);
    }
}

impl CanStore<storage<array<word>>, storage<array<word>>> {
    function store(dst: storage<array<word>>, value: storage<array<word>>) returns () {
        return ();
    }
    function load(dst: storage<array<word>>) returns (storage<array<word>>) {
        return dst;
    }
}

trait Length<self> {
    function length(value: self) returns (uint256);
}

trait Array<self> {
    function pop(value: self) returns ();
}

trait ArrayPush<self, elem> {
    function push(value: self, elem: elem) returns ();
}

impl<t> Length<storage<array<t>>> {
    function length(value: storage<array<t>>) returns (uint256) {
        return uint256(0);
    }
}

impl<t> Array<storage<array<t>>> {
    function pop(value: storage<array<t>>) returns () {
        return ();
    }
}

impl<t, elem> ArrayPush<storage<array<t>>, elem> where storage<t>: CanStore<elem> {
    function push(value: storage<array<t>>, elem: elem) returns () {
        return ();
    }
}
