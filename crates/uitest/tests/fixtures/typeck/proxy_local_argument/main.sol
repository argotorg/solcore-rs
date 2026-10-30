pragma solcore;

enum Proxy<t> {
    Proxy
}

function needsBool(p: Proxy<bool>) {}

function value() {
    let proxy = @word;
    needsBool(proxy);
}
