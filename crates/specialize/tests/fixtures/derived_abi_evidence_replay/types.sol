import * from abi;

export { Leaf(*), Box(*) };

pragma no-generic-instance-for Leaf;

enum Leaf {
    Leaf(word)
}
enum Box {
    Box(Leaf)
}

impl ABIAttribs<Leaf> {
    // Keep the definition-side method observable through specialization. The
    // competing orphan remains pure, so retaining the derived wrapper also
    // proves that evidence was replayed from this module rather than re-solved
    // against every reachable instance.
    function headSize(ty: Proxy<Leaf>) returns (word) {
        assembly {
            sstore(0, 32)
        }
        return 32;
    }
    function isStatic(ty: Proxy<Leaf>) returns (bool) {
        assembly {
            sstore(1, 1)
        }
        return true;
    }
}
