pragma solcore;

type U = word;
type UnitAlias = ();

contract AliasDispatch {
    constructor(seed: U) {}
    fallback() {}
}
