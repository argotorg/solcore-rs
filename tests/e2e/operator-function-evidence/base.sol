pragma solcore;

export { Phantom, Other, ne, lt, le, ge, and, or, not, generic };
trait Phantom<t, r> {
    function pick(x: t) returns (bool);
}
trait Other<t, s> {
    function other(x: t) returns (bool);
}
function ne<t, r, s>(x: t, y: t) returns (bool) where t: Phantom<r>, t: Other<s> {
    return Phantom.pick(x);
}
function lt<t, r, s>(x: t, y: t) returns (bool) where t: Phantom<r>, t: Other<s> {
    return Phantom.pick(x);
}
function le<t, r, s>(x: t, y: t) returns (bool) where t: Phantom<r>, t: Other<s> {
    return Phantom.pick(x);
}
function ge<t, r, s>(x: t, y: t) returns (bool) where t: Phantom<r>, t: Other<s> {
    return Phantom.pick(x);
}
function and<t, r, s>(x: t, y: t) returns (bool) where t: Phantom<r>, t: Other<s> {
    return Phantom.pick(x);
}
function or<t, r, s>(x: t, y: t) returns (bool) where t: Phantom<r>, t: Other<s> {
    return Phantom.pick(x);
}
function not<t, r, s>(x: t) returns (bool) where t: Phantom<r>, t: Other<s> {
    return Other.other(x);
}
function generic<t, r, s>(x: t) returns (word) where t: Phantom<r>, t: Other<s> {
    match (x != x, x < x, x <= x, x >= x,
        (x != x) && (x < x), (x <= x) || (x >= x), !x) {
        case (true, true, true, true, true, true, false) {
            return 3;
        }
        case (false, false, false, false, false, false, true) {
            return 4;
        }
        case (_, _, _, _, _, _, _) {
            return 100;
        }
    }
}
