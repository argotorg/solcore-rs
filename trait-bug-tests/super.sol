pragma solcore;

trait A<t> {
    function fa(x: t) returns (word);
}

trait B<t> where t: A {
    function fb(x: t) returns (word);
}

enum T { T }

// B(T) without A(T): the superclass is never provided.
impl B<T> {
    function fb(x: T) returns (word) {
        return 1;
    }
}

function viaSuper<t>(x: t) returns (word) where t: B {
    return A.fa(x);
}

contract C {
    function main() public returns (word) {
        return viaSuper(T);
    }
}
