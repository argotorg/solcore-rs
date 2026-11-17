pragma solcore;

trait A<t> {
    function fa(x: t) returns (word);
}

trait B<t> where t: A {
    function fb(x: t) returns (word);
}

enum T { T }

// A is satisfied for every type by a default impl.
default impl<t> A<t> {
    function fa(x: t) returns (word) {
        return 7;
    }
}

contract K {
    function main() public returns (word) {
        return A.fa(T);
    }
}
