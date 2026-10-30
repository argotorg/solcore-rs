pragma solcore;

pragma no-patterson-condition;
pragma no-bounded-variable-condition;
pragma no-coverage-condition;

trait Generic<a, rep> {}
trait ABIDeriving<self> {}
trait ABIAttribs<self> {}
trait ABIDecode<decoder, decoded> {}
trait WordReader<reader> {}

enum ABIDecoder<ty, reader> {
    ABIDecoder(reader)
}
enum Reader {
    Reader
}

impl WordReader<Reader> {}
impl ABIAttribs<word> {}
impl ABIDecode<ABIDecoder<word, Reader>, word> {}

pragma no-generic-instance-for Excluded;
enum Eligible {
    Eligible(word)
}
enum Excluded {
    Excluded(word)
}
enum Manual {
    Manual(word)
}
enum Recursive {
    Recursive(Recursive)
}
impl Generic<Manual, word> {}
