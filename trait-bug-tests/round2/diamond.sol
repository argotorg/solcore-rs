pragma solcore;

trait A<t> {
    function fa(x: t) returns (word);
}

trait B<t> where t: A {
    function fb(x: t) returns (word);
}

trait C<t> where t: A {
    function fc(x: t) returns (word);
}

// Both givens entail `t: A`; by coherence they carry the same dictionary.
function g<t>(x: t) returns (word) where t: B, t: C {
    return A.fa(x);
}

enum T { T }

impl A<T> {
    function fa(x: T) returns (word) {
        return 1;
    }
}

impl B<T> {
    function fb(x: T) returns (word) {
        return 2;
    }
}

impl C<T> {
    function fc(x: T) returns (word) {
        return 3;
    }
}

contract K {
    function main() public returns (word) {
        return g(T);
    }
}
