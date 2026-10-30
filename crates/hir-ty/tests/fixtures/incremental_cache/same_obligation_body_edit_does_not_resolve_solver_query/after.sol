pragma solcore;

trait C<a> {}
impl C<word> {}
function use<a>(x: a) returns (word) where a: C {
    return 0;
}

function main() returns (word) {
    let y: word = 2;
    return use(y);
}
