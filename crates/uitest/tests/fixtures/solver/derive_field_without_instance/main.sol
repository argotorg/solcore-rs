pragma solcore;

import * from std;
import * from std.Generic;

// `NoEq` has no `Eq` impl.
enum NoEq {
    NoEq
}

// The derived impl compares the field, so it needs `NoEq: Eq`.
#[derive(Eq)]
enum Wrap {
    Wrap(NoEq)
}

enum List<a> {
    Nil,
    Cons(a)
}

// `a: Eq` does not give `List<a>: Eq`, which the field needs.
#[derive(Eq)]
enum Holder<a> {
    Holder(List<a>)
}

// Every field implements `Eq` once `a` does, so this derive is accepted.
#[derive(Eq)]
enum Fine<a> {
    Fine(a, word)
}

trait Marker<t> {}

// A trait without methods delegates nothing, so its fields need no impl.
#[derive(Marker)]
enum Tagged {
    Tagged(NoEq)
}
