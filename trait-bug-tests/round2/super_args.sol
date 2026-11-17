pragma solcore;

trait A<t, u> {
    function fa(x: t, y: u) returns (word);
}

trait B<t, u> where t: A<u> {
    function fb(x: t, y: u) returns (word);
}

enum T { T }

// A is provided for <T, bool>, but B<T, word> needs A<T, word>.
impl A<T, bool> {
    function fa(x: T, y: bool) returns (word) {
        return 1;
    }
}

impl B<T, word> {
    function fb(x: T, y: word) returns (word) {
        return 2;
    }
}

contract K {
    function main() public returns (word) {
        let w: word = 0;
        return B.fb(T, w);
    }
}
