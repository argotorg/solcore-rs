pragma solcore;

import { Fee } from fees;

export { A(*) };

enum A {
    A
}

impl Fee<A> {
    function fee(x: A) returns (word) {
        return 1;
    }
}
