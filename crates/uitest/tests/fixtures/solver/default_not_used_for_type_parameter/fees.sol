pragma solcore;

export { Fee, charge };

trait Fee<t> {
    function fee(x: t) returns (word);
}

default impl<t> Fee<t> {
    function fee(x: t) returns (word) {
        return 0;
    }
}

// The default impl must not decide `t: Fee` here: a module that calls
// `charge` with its own type may define a more specific impl for it.
function charge<t>(x: t) returns (word) {
    return Fee.fee(x);
}
