pragma solcore;

pragma no-patterson-condition;
pragma no-bounded-variable-condition;
pragma no-generic-instance-for Excluded;

trait Generic<a, rep> {}

enum Eligible {
    Eligible(word)
}
enum Excluded {
    Excluded(word)
}
enum Manual {
    Manual(word)
}

impl Generic<Manual, word> {}
