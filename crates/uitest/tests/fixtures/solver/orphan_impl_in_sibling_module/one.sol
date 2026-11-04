pragma solcore;

import { Fee, A } from fees;

export { useOne };

// Only fees.sol, which defines both `Fee` and `A`, may implement `Fee<A>`.
impl Fee<A> {
    function fee(x: A) returns (word) {
        return 1;
    }
}

function useOne() returns (word) {
    return Fee.fee(A);
}
