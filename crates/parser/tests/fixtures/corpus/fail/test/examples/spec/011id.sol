pragma solcore;

contract Id1 {

    enum Bool {
        False,
        True
    }

    function id<a>(x: a) public {
        return x;
    }

    function const<a, b>(x: a, y: b) public {
        return x;
    }

    function main() public {
        return const(id(42), Bool.False);
    }
}
