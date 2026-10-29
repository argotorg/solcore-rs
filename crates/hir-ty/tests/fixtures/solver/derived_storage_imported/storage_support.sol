pragma solcore;

pragma no-patterson-condition;
pragma no-bounded-variable-condition;
pragma no-coverage-condition;

export { Generic, StorageDeriving, StorageSize, CanStore, storage(*) };

trait Generic<a, rep> {}
trait StorageDeriving<self> {}
trait StorageSize<self> {}
trait CanStore<slot, value> {}

enum storage<ty> {
    storage(word)
}

impl StorageSize<word> {}
impl CanStore<storage<word>, word> {}
