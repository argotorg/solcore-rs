import * from std;

enum Box {
    Box(word)
}

impl StorageType<Box> {
    function load(ptr: word) returns (Box) {
        return Box(StorageType.load(ptr));
    }

    function store(ptr: word, value: Box) {
        match (value) {
            case Box(inner) {
                StorageType.store(ptr, inner);
            }
        }
    }
}

impl CanStore<storage<Box>, Box> {
    function load(ptr: storage<Box>) returns (Box) {
        return StorageType.load(Typedef.rep(ptr));
    }

    function store(ptr: storage<Box>, value: Box) {
        StorageType.store(Typedef.rep(ptr), value);
    }
}

contract DataTypeStorageFull {
    box: Box;

    function main() public returns (word) {
        match (box) {
            case Box(inner) {
                return inner;
            }
        }
    }
}
