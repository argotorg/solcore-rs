pragma solcore;
pragma coherence-wrapper Wrapped;

export { Fee, Wrapped(*), charge, chargePair, chargeWrapped };

trait Fee<t> {
    function fee(x: t) returns (word);
}

enum Wrapped<a> {
    Wrapped(a)
}

default impl<t> Fee<t> {
    function fee(x: t) returns (word) {
        return 0;
    }
}

// The solver matches `comptime<t>` as `t`, so the default impl must not decide
// these goals any more than it decides `t: Fee` for a run-time `t`: a module
// that calls them with its own type may define a more specific impl.
function charge<t>(comptime x: t) returns (word) {
    return Fee.fee(x);
}

function chargePair<t>(comptime x: t) returns (word) {
    return Fee.fee((x, x));
}

function chargeWrapped<t>(comptime x: t) returns (word) {
    return Fee.fee(Wrapped(x));
}
