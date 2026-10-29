pragma solcore;

contract Box<t> {
    enum Option<u> {
        None,
        Some(u)
    }

    function mk(x: word) returns (Option<word>) {
        return .Some(x);
    }

    function main() {
        return ();
    }
}
