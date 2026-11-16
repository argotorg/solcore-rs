pragma solcore;

import { Get } from table;

enum Squares {
    Squares
}

enum Key {
    Key
}

// A tuple belongs with its first element, so this module may implement `Get`
// for `(Squares, i)`.
impl<i> Get<(Squares, i), word> {
    function get(ci: (Squares, i)) returns (word) {
        return 1;
    }
}

// The first element is `word`, so only `table` may implement `Get` for this
// tuple, even though `Key` is defined here.
impl Get<(word, Key), word> {
    function get(ci: (word, Key)) returns (word) {
        return 2;
    }
}

function main() returns (word) {
    return Get.get((Squares, Key));
}
