pragma solcore;

import * from std;

trait Same<t> {
    function same(x: t, y: t) returns (bool);
}

enum Box<a> { Box(a) }

// The impl states both `a: Eq` and `a: Ord`; its method uses `Eq`.
impl<a> Same<Box<a>> where a: Eq, a: Ord {
    function same(x: Box<a>, y: Box<a>) returns (bool) {
        match (x, y) {
            case (Box(p), Box(q)) {
                return Eq.eq(p, q);
            }
        }
    }
}
