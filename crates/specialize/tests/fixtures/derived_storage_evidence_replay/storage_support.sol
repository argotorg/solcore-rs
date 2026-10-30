pragma solcore;

pragma no-patterson-condition;
pragma no-bounded-variable-condition;
pragma no-coverage-condition;

export {
    Generic,
    StorageDeriving,
    Proxy(*),
    storage(*),
    StorageSize,
    CanStore
};

enum Proxy<t> {
    Proxy
}
enum storage<t> {
    storage(word)
}

trait Generic<a, rep> {
    function from(x: a) returns (rep);
    function to(x: rep) returns (a);
}
trait StorageDeriving<self> {}
trait StorageSize<self> {
    function size(x: Proxy<self>) returns (word);
}
trait CanStore<slot, value> {
    function store(r: slot, v: value);
    function load(r: slot) returns (value);
}

impl StorageSize<word> {
    function size(x: Proxy<word>) returns (word) {
        assembly {
            sstore(0, 1)
        }
        return 1;
    }
}

impl CanStore<storage<word>, word> {
    function store(r: storage<word>, v: word) {
        match (r) {
            case storage(slot) {
                assembly {
                    sstore(slot, v)
                }
            }
        }
    }
    function load(r: storage<word>) returns (word) {
        match (r) {
            case storage(slot) {
                let result: word;
                assembly {
                    result := sload(slot)
                }
                return result;
            }
        }
    }
}
