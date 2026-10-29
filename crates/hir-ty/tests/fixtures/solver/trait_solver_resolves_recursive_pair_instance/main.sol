pragma solcore;

enum Pair<a, b> {
    Pair(a, b)
}

trait StorageSize<a> {}

impl StorageSize<word> {}

impl<a, b> StorageSize<Pair<a, b>> where a: StorageSize, b: StorageSize {}
