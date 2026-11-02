pragma solcore;

export { Fee, A(*) };

trait Fee<t> {
    function fee(x: t) returns (word);
}

enum A {
    A
}
