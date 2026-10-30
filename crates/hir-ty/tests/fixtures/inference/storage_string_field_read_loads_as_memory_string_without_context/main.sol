pragma solcore;

enum string {}
enum memory<t> {
    memory(word)
}
enum storage<t> {
    storage(word)
}

trait CanStore<a, b> {
    function store(r: a, v: b) returns ();
    function load(r: a) returns (b);
}

impl CanStore<storage<string>, memory<string>> {
    function store(dst: storage<string>, src: memory<string>) returns () {
        return ();
    }

    function load(src: storage<string>) returns (memory<string>) {
        return memory(0);
    }
}

contract C {
    value: string;

    function get() {
        let x = value;
        return ();
    }
}
