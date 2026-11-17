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

enum Box<a> { Box(a) }

impl<a> A<Box<a>> where a: A {
    function fa(x: Box<a>) returns (word) {
        return 1;
    }
}

// The superclass A<Box<a>> needs `a: A`, which both conditions entail.
impl<a> B<Box<a>> where a: B, a: C {
    function fb(x: Box<a>) returns (word) {
        return 2;
    }
}
