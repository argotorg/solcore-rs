function leak<a>() returns (a) {
    let y: a;
    return y;
}

contract C {
    function main() public {
        let x = leak();
        return ();
    }
}
