pragma solcore;

import { useOne } from one;
import { useTwo } from two;

// Each sibling sees only its own impl, so without the placement rule the same
// call `Fee.fee(A)` would return 1 in one module and 2 in the other.
function main() returns (word) {
    return primAddWord(useOne(), useTwo());
}
