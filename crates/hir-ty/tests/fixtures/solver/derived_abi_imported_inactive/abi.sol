pragma solcore;

pragma no-patterson-condition;
pragma no-bounded-variable-condition;
pragma no-coverage-condition;

import { Generic } from generic;

export {
    ABIDeriving,
    ABIAttribs,
    ABIDecode,
    WordReader,
    ABIDecoder(*),
    Reader
};

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
