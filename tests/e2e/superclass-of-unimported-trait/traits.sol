pragma solcore;

export { Base, Refined };

trait Base<t> {
    function base(x: t) returns (word);
}

trait Refined<t> where t: Base {
    function refined(x: t) returns (word);
}
