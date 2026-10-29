pragma solcore;

trait Typedef<abs, rep> {
    function abs(x: rep) returns (abs);
    function rep(x: abs) returns (rep);
}

impl<t> Typedef<t, t> {
    function abs(x: t) returns (t) {
        return x;
    }
    function rep(x: t) returns (t) {
        return x;
    }
}

function lift1ac<abs, rep, res>(f: function (rep) returns (res), x: rep) returns (res) where abs: Typedef<rep> {
    f(Typedef.rep(x))
}
