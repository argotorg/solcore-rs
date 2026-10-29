pragma solcore;

enum W<a> {
    W(a)
}
trait Foo<a> {
    function foo();
}
impl Foo<(word, W<a>)> {
    function foo() {}
}
