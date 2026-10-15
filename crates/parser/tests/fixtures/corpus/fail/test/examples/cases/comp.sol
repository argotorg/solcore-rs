function compose<a, b, c>(f: function (b) returns (c), g: function (a) returns (b), x: a) {
    return f(g(x));
}
