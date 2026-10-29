pragma solcore;

pragma no-patterson-condition Assign;

enum storage<t> {
    storage(word)
}
enum mapping<k, v> {
    mapping(word)
}
enum Foo {
    Foo(word)
}

trait CanStore<dst, value> {
    function store(dst: dst, value: value) returns ();
    function load(dst: dst) returns (value);
}

trait Assign<lhs, rhs> {
    function assign(lhs: lhs, rhs: rhs) returns ();
}

impl CanStore<storage<word>, word> {
    function store(dst: storage<word>, value: word) returns () {
        return ();
    }
    function load(dst: storage<word>) returns (word) {
        return 0;
    }
}

impl<a, b> Assign<a, b> where a: CanStore<b> {
    function assign(lhs: a, rhs: b) returns () {
        CanStore.store(lhs, rhs);
    }
}

impl Assign<storage<word>, bool> {
    function assign(lhs: storage<word>, rhs: bool) returns () {
        return ();
    }
}

impl CanStore<storage<mapping(word => word)>, storage<mapping(word => word)>> {
    function store(dst: storage<mapping(word => word)>, value: storage<mapping(word => word)>) {
        return ();
    }
    function load(dst: storage<mapping(word => word)>) returns (storage<mapping(word => word)>) {
        return storage(0);
    }
}

impl Assign<storage<mapping(word => word)>, Foo> {
    function assign(lhs: storage<mapping(word => word)>, rhs: Foo) returns () {
        return ();
    }
}

contract C {
    value: word;
    values: mapping(word => word);

    function write(flag: bool) returns () {
        value = flag;
        return ();
    }

    function writeAnnotated(flag: bool) returns () {
        value = flag;
        return ();
    }

    function writeMapping(value: Foo) returns () {
        values = value;
        return ();
    }
}
