pragma solcore;

trait A<t> {
    function fa(x: t) returns (word);
}

enum Box<a> { Box(a) }

// The superclass constrains `Box<t>`, not `t`.
trait B<t> where Box<t>: A {
    function fb(x: t) returns (word);
}

enum T { T }

// No impl provides `Box<T>: A`.
impl B<T> {
    function fb(x: T) returns (word) {
        return 1;
    }
}

function g<t>(x: t) returns (word) where t: B {
    return A.fa(Box(x));
}

contract K {
    function main() public returns (word) {
        return g(T);
    }
}
