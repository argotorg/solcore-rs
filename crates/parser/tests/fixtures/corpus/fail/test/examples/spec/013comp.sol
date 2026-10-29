pragma solcore;

contract Compose {
    function compose<a, b, c>(f: function (b) returns (c), g: function (a) returns (b)) public {
        return lam (x) {
            return f(g(x));
        };
    }

    function id<a>(x: a) public {
        return x;
    }

    function idid() public {
        return compose(id, id);
    }

    function main() public {
        let f = compose(id, id);
        return f(42);
    }
}
