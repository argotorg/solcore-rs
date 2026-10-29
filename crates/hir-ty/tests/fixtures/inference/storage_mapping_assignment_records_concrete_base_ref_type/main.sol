pragma solcore;

enum mapping<index, member> {
    mapping(word)
}
enum storage<t> {
    storage(word)
}

trait CanStore<a, b> {
    function store(r: a, v: b) returns ();
    function load(r: a) returns (b);
}

impl CanStore<storage<word>, word> {
    function store(dst: storage<word>, src: word) returns () {
        return ();
    }

    function load(src: storage<word>) returns (word) {
        return 0;
    }
}

contract C {
    m: mapping(word => word);

    function next() returns (word) {
        return 1;
    }

    function main() {
        m[next()] = next();
    }
}
