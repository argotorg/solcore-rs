pragma solcore;

trait IsA<b> {
    function ais<a>(x: a, witness: b) returns (a);
}

impl IsA<word> {
    function ais<a>(x: a, witness: word) returns (a) {
        return x;
    }
}
