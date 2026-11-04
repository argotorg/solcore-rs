pragma solcore;

import { Fee, A } from fees;

export { useTwo };

impl Fee<A> {
    function fee(x: A) returns (word) {
        return 2;
    }
}

function useTwo() returns (word) {
    return Fee.fee(A);
}
