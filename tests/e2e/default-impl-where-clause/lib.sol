pragma solcore;

export { A, viaCt };

trait A<t> {
    function fa(x: t) returns (word);
}

default impl<t> A<t> {
    function fa(x: t) returns (word) {
        return 7;
    }
}

// The default impl may not decide `t: A` for a `comptime` parameter. With the
// constraint stated, the caller's impl for its own type is used.
function viaCt<t>(comptime x: t) returns (word) where t: A {
    return A.fa(x);
}
