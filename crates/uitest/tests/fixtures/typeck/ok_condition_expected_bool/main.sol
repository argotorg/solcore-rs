pragma solcore;

function conditions(x: bool) returns (bool) {
    if (.true) {} else {}
    while (.false) {}
    for (; .false;) {}
    return .true ? x : .false;
}
