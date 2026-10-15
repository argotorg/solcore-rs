enum Proxy<t> {
    Proxy(word)
}

function value() returns (Proxy<word>) {
    return @word;
}
