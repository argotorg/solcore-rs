pragma solcore;

function f() {
    let self = lam (x) {
        return x(x);
    };
    return ();
}
