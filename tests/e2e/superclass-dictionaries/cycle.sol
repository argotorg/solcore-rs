pragma solcore;

export { cycle };
trait CycleParent<t> where t: CycleChild {
    function f(x: t) returns (word);
}
trait CycleChild<t> where t: CycleParent {
    function g(x: t) returns (word);
}
impl CycleParent<word> {
    function f(x: word) returns (word) {
        return 1;
    }
}
impl CycleChild<word> {
    function g(x: word) returns (word) {
        return 2;
    }
}
function decrement(n: word) returns (word) {
    let result: word;
    assembly {
        result := sub(n, 1)
    }
    return result;
}
function recurse<t>(x: t, n: word) returns (word) where t: CycleChild {
    match (n) {
        case (0) {
            return CycleParent.f(x);
        }
        case (_) {
            return viaParent(x, decrement(n));
        }
    }
}
function viaParent<t>(x: t, n: word) returns (word) where t: CycleParent {
    return recurse(x, n);
}
function cycle(n: word) returns (word) {
    let x: word = 0;
    return recurse(x, n);
}
