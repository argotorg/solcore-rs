contract Id1 {
    function id<a>(x: a) public {
        return x;
    }

    function main() public {
        let nid = lam (x) {
            return x;
        };
        return nid(42);
    }
}
