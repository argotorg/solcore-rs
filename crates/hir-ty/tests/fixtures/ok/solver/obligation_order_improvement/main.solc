// Regression test for fixpoint obligation solving with class-argument
// improvement (mirrors the reference's TcSimplify `toHnfs` fixpoint).
//
// `Assign2.assign(Mk.mk(S), 7)` pushes the callee obligation
// `?lhs:Assign2(word)` BEFORE the argument obligation `S:Mk(?o)`. A single
// in-order pass rejects the var-headed Assign2 goal (SC0207); the fixpoint
// solver defers it, solves `S:Mk(?o)` (pinning ?o := R(word) via
// class-argument unification), and then discharges the improved goal
// `R(word):Assign2(word)` in the next round. The reference compiler accepts
// this program.

forall lhs rhs .
class lhs:Assign2(rhs) {
    function assign(l:lhs, r:rhs) -> ();
}

forall s o .
class s:Mk(o) {
    function mk(x:s) -> o;
}

data R(a) = R(a);

forall a .
instance R(a):Assign2(a) {
    function assign(l:R(a), r:a) -> () {
        return ();
    }
}

data S = S;

instance S:Mk(R(word)) {
    function mk(x:S) -> R(word) {
        return R(0);
    }
}

contract Main {
    public function main() -> word {
        Assign2.assign(Mk.mk(S), 7);
        return 1;
    }
}
