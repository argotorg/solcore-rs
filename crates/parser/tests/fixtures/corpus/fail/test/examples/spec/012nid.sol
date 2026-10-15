contract Id1 {
    function id<a>(x: a) public {
        return x;
    }

    function nid() public {
        return id;
    }

    function const<a, b>(x: a, y: b) public {
        return x;
    }

    function main() public {
        return const(nid(42), id(1));
    }
}
