pragma solcore;

import * from std;

enum NoEq {
    NoEq
}

// A derived method goes through the Generic representation, so deriving needs
// `std.Generic` in scope even when the field type has the trait.
#[derive(Eq)]
enum Missing {
    Missing(NoEq)
}

#[derive(Eq)]
enum Plain {
    Plain(word)
}

// An empty type does not use Generic.
#[derive(Eq)]
enum Never {}

trait Marker<t> {}

// A trait without methods delegates nothing to Generic.
#[derive(Marker)]
enum Tagged {
    Tagged(word)
}
