pragma solcore;

pragma no-patterson-condition;
pragma no-bounded-variable-condition;
pragma no-coverage-condition;

import { Generic } from generic;

export { StorageDeriving, StorageSize, CanStore, storage(*) };

trait StorageDeriving<self> {}
trait StorageSize<self> {}
trait CanStore<slot, value> {}

enum storage<ty> {
    storage(word)
}

impl StorageSize<word> {}
impl CanStore<storage<word>, word> {}
