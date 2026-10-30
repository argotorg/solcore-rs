pragma solcore;

enum Pair<a, b> {
    Pair(a, b)
}

trait D<a, r> {}
default impl<a> D<a, word> {}
default impl<a> D<a, bool> {}

trait C<a> {}
impl<a, r> C<Pair<a, r>> where a: D<r> {}
