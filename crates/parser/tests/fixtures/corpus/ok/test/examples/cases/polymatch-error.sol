pragma solcore;

function fst<a, b>(p: (a, b)) returns (a) {
    match (p) {
        case (a, _) {
            return a;
        }
    }
}
contract TestUnitMatch {
    function main() public {
        match (((), ())) {
            case x {
                return fst(x);
            }
        }
    }
}
