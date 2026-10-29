pragma solcore;

//~ specialize: loader bare

trait C<t> {
    function f<u>(x: t, y: u) returns (u);
}
impl C<word> {
    function f<u>(x: word, y: u) returns (u) {
        assembly {
            mstore(0, x)
        }
        return y;
    }
}
function main(x: word, y: word) returns (word) {
    let scalar: word = C.f(x, y);
    let pair: (word, word) = C.f(x, (x, y));
    match (pair) {
        case ((p, q)) {
            return primAddWord(scalar, q);
        }
    }
}
