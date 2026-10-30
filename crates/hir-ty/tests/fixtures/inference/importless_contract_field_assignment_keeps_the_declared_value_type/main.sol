pragma solcore;

contract C {
    value: word;

    function write(flag: bool) returns () {
        value = flag;
        return ();
    }
}
