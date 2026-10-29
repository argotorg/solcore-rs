pragma solcore;

pragma no-patterson-condition;
pragma no-bounded-variable-condition;
pragma no-coverage-condition;

export {
    Generic,
    ABIDeriving,
    Proxy(*),
    ABIAttribs,
    ABIDecode,
    WordReader,
    ABIDecoder(*)
};

enum Proxy<t> {
    Proxy
}
enum ABIDecoder<ty, reader> {
    ABIDecoder(reader)
}

trait Generic<a, rep> {
    function from(x: a) returns (rep);
    function to(x: rep) returns (a);
}
trait ABIDeriving<self> {}
trait ABIAttribs<self> {
    function headSize(ty: Proxy<self>) returns (word);
    function isStatic(ty: Proxy<self>) returns (bool);
}
trait ABIDecode<decoder, decoded> {
    function decode(ptr: decoder, headOffset: word) returns (decoded);
}
trait WordReader<reader> {}
