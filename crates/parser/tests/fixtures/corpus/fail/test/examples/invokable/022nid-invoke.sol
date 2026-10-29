pragma solcore;

trait Invokable<self, args, ret> {
    function invoke(s: self, a: args) returns (ret);
}

function id<a>(x: a) returns (a) {
    return x;
}

enum IdToken<a> {
    IdToken
}

impl Invokable<IdToken<a>, a, a> {
    function invoke(token: IdToken<a>, arg: a) returns (a) {
        return id(arg);
    }
}

contract InvokeId {
    function id<a>(x: a) public returns (a) {
        return x;
    }

    /*
    function nid() {
    return id;
  }
  */

    function nidimpl<a>() public returns (IdToken<a>) {
        return IdToken;
    }

    function main() public returns (word) {
        // Instead of: `return nid(42)`
        return invoke(nidimpl(), 42);
    }
}
