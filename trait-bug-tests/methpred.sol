pragma solcore;

trait C<t> {
    function c(x: t) returns (word);
}

trait D<t> {
    function d(x: t) returns (word);
}

enum Box<a> { Box(a) }
enum T { T }

// The method requires `a: D`, which neither the trait nor the impl head states.
impl<a> C<Box<a>> {
    function c(x: Box<a>) returns (word) where a: D {
        match (x) {
            case Box(inner) {
                return D.d(inner);
            }
        }
    }
}

// T has no D impl.
function f() returns (word) {
    return C.c(Box(T));
}

contract K {
    function main() public returns (word) {
        return f();
    }
}
