import * from std;
import * from std.dispatch;

// #[scenario("forloops")]
// constructor()
// #[deploy(0x) -> success]
// #[raw(0xc5d339a8, label = "break_sum()(uint256) -> 0+1+2+3+4 = 10") -> success(0x000000000000000000000000000000000000000000000000000000000000000a)]
// #[raw(0xe26b41bc, label = "continue_sum()(uint256) -> 5+6+7+8+9 = 35") -> success(0x0000000000000000000000000000000000000000000000000000000000000023)]
// #[raw(0x73c5c5b9, label = "empty_init()(uint256) -> 3+4+5+6 = 18") -> success(0x0000000000000000000000000000000000000000000000000000000000000012)]
// #[raw(0xf2efd0fe, label = "empty_post()(uint256) -> 0+1+2+3 = 6") -> success(0x0000000000000000000000000000000000000000000000000000000000000006)]
// #[raw(0xf837ea36, label = "cond_side_effect()(uint256) -> bump() called until it returns 5 -> counter = 5") -> success(0x0000000000000000000000000000000000000000000000000000000000000005)]
// #[raw(0x846ca1fa, label = "post_side_effect()(uint256) -> bump() runs once per iteration, 3 iterations -> counter = 3") -> success(0x0000000000000000000000000000000000000000000000000000000000000003)]
// #[raw(0xe819651a, label = "double_loop()(uint256) -> sum_{i,j in 1..3} i*j = 36") -> success(0x0000000000000000000000000000000000000000000000000000000000000024)]
// #[raw(0x8ada066e, label = "getCounter()(uint256) -> last call was post_side_effect, so counter = 3") -> success(0x0000000000000000000000000000000000000000000000000000000000000003)]
contract C {
    counter : uint256;

    constructor() {
        counter = uint256(0);
    }

    function bump() returns (uint256) {
        counter = counter + uint256(1);
        return counter;
    }

    function getCounter() public returns (uint256) {
        return counter;
    }

    // Sum of 0..4 with early `break` at i == 5.
    function break_sum() public returns (uint256) {
        let s : uint256 = uint256(0);
        for (let i : uint256 = uint256(0); i < uint256(10); i = i + uint256(1)) {
            if (i == uint256(5)) {
                break;
            } else {}
            s = s + i;
        }
        return s;
    }

    // Sum of 5..9 using `continue` to skip the iterations where i < 5.
    // The post-statement (i = i + 1) must still run on `continue`, otherwise
    // the loop would never terminate.
    function continue_sum() public returns (uint256) {
        let s : uint256 = uint256(0);
        for (let i : uint256 = uint256(0); i < uint256(10); i = i + uint256(1)) {
            if (i < uint256(5)) {
                continue;
            } else {}
            s = s + i;
        }
        return s;
    }

    // Empty initializer: `i` is declared/initialised outside the loop.
    function empty_init() public returns (uint256) {
        let i : uint256 = uint256(3);
        let s : uint256 = uint256(0);
        for (; i < uint256(7); i = i + uint256(1)) {
            s = s + i;
        }
        return s;
    }

    // Empty post-body: the increment is done in the loop body.
    function empty_post() public returns (uint256) {
        let s : uint256 = uint256(0);
        for (let i : uint256 = uint256(0); i < uint256(4); ) {
            s = s + i;
            i = i + uint256(1);
        }
        return s;
    }

    // Side effect in the condition: `bump()` increments storage on every
    // probe (including the failing one), so observing `counter` afterwards
    // proves the condition ran the expected number of times.
    function cond_side_effect() public returns (uint256) {
        counter = uint256(0);
        for (let i : uint256 = uint256(0); bump() < uint256(5); i = i + uint256(1)) {}
        return counter;
    }

    // Side effect in the post-body: `bump()` runs once per completed
    // iteration, so `counter` ends equal to the iteration count.
    function post_side_effect() public returns (uint256) {
        counter = uint256(0);
        for (let i : uint256 = uint256(0); i < uint256(3); bump()) {
            i = i + uint256(1);
        }
        return counter;
    }

    // Nested `for` -- sum of i*j for i,j in 1..3.
    function double_loop() public returns (uint256) {
        let s : uint256 = uint256(0);
        for (let i : uint256 = uint256(1); i < uint256(4); i = i + uint256(1)) {
            for (let j : uint256 = uint256(1); j < uint256(4); j = j + uint256(1)) {
                s = s + i * j;
            }
        }
        return s;
    }
}
