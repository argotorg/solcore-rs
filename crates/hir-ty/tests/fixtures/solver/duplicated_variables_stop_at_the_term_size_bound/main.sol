pragma solcore;

pragma no-patterson-condition;

// Every subgoal pairs the previous one with itself: `word`, `(word, word)`,
// `((word, word), (word, word))`, ... doubles in size at each step.
trait C<a> {}

impl<a> C<a> where(a, a): C {}
