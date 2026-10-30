pragma solcore;

import * from std;

export {
    Contract(*),
    Fallback(*),
    Method(*),
    NonPayable,
    Payable,
    RunContract,
    SigString,
    fallback_default_implementation
};

enum Contract<methods, fb> {
    Contract(methods, fb)
}
enum Method<name, payability, args, rets, fn> {
    Method(Proxy<name>, Proxy<payability>, Proxy<args>, Proxy<rets>, fn)
}
enum Fallback<payability, args, rets, fn> {
    Fallback(Proxy<payability>, Proxy<args>, Proxy<rets>, fn)
}
enum Payable {}
enum NonPayable {}

trait SigString<t> {
    function sigStr(value: Proxy<t>) returns (string);
}

trait RunContract<c> {
    function exec(value: c) returns ();
}

impl<name, payability, args, rets, fn, fb>
RunContract<Contract<Method<name, payability, args, rets, fn>, fb>>
where name: SigString {
    function exec(value: Contract<Method<name, payability, args, rets, fn>, fb>) returns () {
        return ();
    }
}

function fallback_default_implementation() returns () {
    return ();
}
