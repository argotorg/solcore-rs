pragma solcore;

// Bug: local variable named `rep` causes name capture with the type variable `rep`
// from `class abs : Typedef(rep)`. Resolution must retain the qualifier
// when distinguishing local variables from class methods. Ignoring it caused
// a qualified call `Typedef.rep(a)` to resolve to the
// local variable `rep` instead of the class method.
//
// Expected: compiles successfully; `Typedef.rep` resolves to the class method.
// Actual (before fix): PANIC: no resolution found for invokable.invoke

import * from std;
import * from std.dispatch;
pragma no-patterson-condition;
pragma no-coverage-condition;
pragma no-bounded-variable-condition;

contract Bug {
    constructor() {}

    function f(a: uint256) returns (uint256) {
        let rep: uint256 = a;
        let w: word = Typedef.rep(a);
        return Typedef.abs(w);
    }
}
