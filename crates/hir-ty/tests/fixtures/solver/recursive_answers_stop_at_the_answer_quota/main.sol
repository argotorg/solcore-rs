pragma solcore;

pragma no-coverage-condition;

// With an assumed `word: M<t>`, the goal `word: M<?b>` has the answers
// `t`, `Box<t>`, `Box<Box<t>>`, ...: the impl wraps each answer again.
enum Box<a> {
    MkBox(a)
}

trait M<self, b> {}

impl<a> M<word, Box<a>> where word: M<a> {}
