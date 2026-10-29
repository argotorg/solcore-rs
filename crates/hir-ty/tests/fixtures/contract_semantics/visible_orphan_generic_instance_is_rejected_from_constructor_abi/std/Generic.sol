pragma solcore;

pragma no-patterson-condition;
pragma no-bounded-variable-condition;
export { Generic };
trait Generic<a, rep> {
    function from(x: a) returns (rep);
    function to(x: rep) returns (a);
}
