pragma solcore;

trait A<a> {
    function f(x: a) returns (word);
}
trait B<a> where a: A {}
trait C<a> where a: A {}
trait Run<a> {
    function run(x: a) returns (word);
}
enum Box<a> {
    Box(a)
}
impl<a> A<Box<a>> where a: A {
    function f(x: Box<a>) returns (word) {
        match (x) {
            case Box(inner) {
                return A.f(inner);
            }
        }
    }
}
impl<a> B<Box<a>> where a: B, a: C {}
impl<a> Run<Box<a>> where a: B, a: C {
    function run(x: Box<a>) returns (word) {
        match (x) {
            case Box(inner) {
                return A.f(inner);
            }
        }
    }
}
impl A<word> {
    function f(x: word) returns (word) {
        return 7;
    }
}
impl B<word> {}
impl C<word> {}
function main() returns (word) {
    let x: word = 0;
    return Run.run(Box(x));
}
