pragma solcore;

import { memory, storage, CanStore } from std;

enum Blob {}

impl CanStore<storage<Blob>, memory<Blob>> {
    function store(dst: storage<Blob>, value: memory<Blob>) returns () {
        return ();
    }
    function load(dst: storage<Blob>) returns (memory<Blob>) {
        return memory(0);
    }
}

contract C {
    value: Blob;

    function write(src: memory<Blob>) returns () {
        value = src;
        return ();
    }

    function read() returns (memory<Blob>) {
        return value;
    }
}
