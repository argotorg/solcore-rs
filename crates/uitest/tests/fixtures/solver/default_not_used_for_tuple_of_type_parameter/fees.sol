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

// A tuple belongs with its first element, so the default impl must not decide
// `(t, t): Fee` either: a module that calls `charge` with its own type may
// define a more specific impl for the tuple.
function charge<t>(x: t) returns (word) {
    return Fee.fee((x, x));
}
