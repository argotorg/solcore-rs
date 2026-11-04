pragma solcore;

trait Size<a> {
    function size(x: a) returns (word);
}

enum Box<a> {
    Box(a)
}

// Both defaults apply to every `Box<a>`, and the second has no condition that
// could tell them apart, so a goal like `Box<word>: Size` would be ambiguous.
default impl<a> Size<a> {
    function size(x: a) returns (word) {
        return 32;
    }
}

default impl<a> Size<Box<a>> {
    function size(x: Box<a>) returns (word) {
        return 64;
    }
}
