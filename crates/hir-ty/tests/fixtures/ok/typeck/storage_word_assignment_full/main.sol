pragma solcore;

enum storage<t> {
    storage(word)
}

trait CanStore<a, b> {
    function store(r: a, v: b);
    function load(r: a) returns (b);
}

impl CanStore<storage<word>, word> {
    function store(dst: storage<word>, src: word) {
        return ();
    }

    function load(src: storage<word>) returns (word) {
        return 0;
    }
}

contract StorageWordAssign {
    x: word;

    function setx() {
        x = 8;
    }

    function main() public returns (word) {
        setx();
        return x;
    }
}
