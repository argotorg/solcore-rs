pragma solcore;

import * from std;
import * from std.dispatch;

// One given reaches `v: A` three times: through `T5` and `T1`, through `T3`
// and `T1`, and through `T4`. `T4` declares its superclass as
// `comptime<t>: A`, a dictionary that a run-time call cannot use. The proofs
// are one dictionary, and the one that inference keeps has to be usable.
trait A<t> {
    function fa(x: t) returns (word);
}

trait T5<t> where t: T1 {}

trait T1<t> where t: A {}

trait X1<t> where t: A {}

trait X2<t> where t: A {}

trait T3<t> where t: T1 {}

trait T6<t> where t: T5, t: T4, t: T3 {}

trait T4<t> where comptime<t>: A {}

impl A<word> {
    function fa(x: word) returns (word) {
        return 7;
    }
}

impl T1<word> {}

impl T5<word> {}

impl T3<word> {}

impl T4<word> {}

impl T6<word> {}

function pick<v>(x: v) returns (word) where v: T6 {
    return A.fa(x);
}

contract GivenDiamondSuperclass {
    // #[() -> 7]
    function throughOneGiven() public returns (uint256) {
        let n: word = 0;
        return uint256(pick(n));
    }
}
