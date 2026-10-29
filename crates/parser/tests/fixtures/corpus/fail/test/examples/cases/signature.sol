pragma solcore;

trait Typedef<self, underlyingType> {
    function rep(x: self) returns (underlyingType);
}

function tripleFun<t>(x: t) returns (word) where t: Typedef<word> {
    return Typedef.rep(x);
}
