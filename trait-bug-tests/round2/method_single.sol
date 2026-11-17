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

trait M<t> {
    function m(x: t) returns (word);
}

enum Box<a> { Box(a) }

// The method asks for `a: A`, which both impl conditions entail.
impl<a> M<Box<a>> where a: B {
    function m(x: Box<a>) returns (word) where a: A {
        return 0;
    }
}
